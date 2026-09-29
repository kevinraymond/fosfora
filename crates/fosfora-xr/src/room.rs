//! S7 mixed reality: the passthrough layer (`XR_FB_passthrough`) and the
//! room's scene anchors (`XR_FB_scene` over `XR_FB_spatial_entity_query`)
//! turned into oriented boxes for the particle sim's obstacle test.
//!
//! The scene route goes through `openxr-sys` function pointers because the
//! `openxr` crate has no safe wrapper for the FB spatial-entity family. Every
//! call follows the OpenXR spec's two-call idiom and checks the returned
//! `XrResult`; the anchor `XrSpace` handles are destroyed on drop.

use std::ffi::{CStr, c_char};
use std::ptr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use log::{info, warn};
use openxr as xr;
use xr::sys;
use xr::sys::Handle as _;

use crate::particles3d::ObstacleBox;

/// Passthrough: a reconstruction layer that the frame loop submits under the
/// projection layer. Created running, so the `openxr` crate's `start()`
/// (which calls `xrPassthroughPauseFB` in 0.22) is never needed.
pub struct Passthrough {
    // Layer before the feature: fields drop in order and the layer belongs
    // to the passthrough.
    pub layer: xr::PassthroughLayerFB,
    _passthrough: xr::Passthrough,
}

impl Passthrough {
    pub fn new(session: &xr::Session<xr::Vulkan>) -> Result<Self> {
        let passthrough = session
            .create_passthrough(xr::PassthroughFlagsFB::IS_RUNNING_AT_CREATION)
            .context("xrCreatePassthroughFB")?;
        let layer = session
            .create_passthrough_layer(
                &passthrough,
                xr::PassthroughFlagsFB::IS_RUNNING_AT_CREATION,
                xr::PassthroughLayerPurposeFB::RECONSTRUCTION,
            )
            .context("xrCreatePassthroughLayerFB")?;
        info!("passthrough: reconstruction layer created and running");
        Ok(Self {
            layer,
            _passthrough: passthrough,
        })
    }
}

/// What `debug.fosfora.rescan` asks for once the room is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rescan {
    #[default]
    Off,
    /// Space Setup, then the query (the hand menu's action).
    Capture,
    /// The query alone.
    Query,
}

/// Half thickness given to a 2D scene plane so fast particles cannot tunnel
/// through it in one step (meters).
const PLANE_HALF_THICKNESS_M: f32 = 0.02;
/// Anchors are static in the stage space; relocating them this often is
/// plenty and keeps the per-frame cost at zero.
const RELOCATE_EVERY: Duration = Duration::from_secs(1);
const MAX_RESULTS: u32 = 64;
/// Empty query results tolerated (retried every `QUERY_RETRY_EVERY`) before
/// the room is taken to be missing: right after the session focuses the
/// runtime returned 0 anchors for a room that a later query found (Sep 27).
const EMPTY_RESULTS_BEFORE_CAPTURE: u32 = 6;
const QUERY_RETRY_EVERY: Duration = Duration::from_secs(3);
/// Labels the query asks the runtime to report as-is (spec: without this
/// list newer labels are folded into legacy ones).
const RECOGNIZED_LABELS: &CStr = c"DESK,COUCH,FLOOR,CEILING,WALL_FACE,WINDOW_FRAME,DOOR_FRAME,STORAGE,BED,SCREEN,LAMP,PLANT,TABLE,OTHER,INVISIBLE_WALL_FACE,WALL_ART,GLOBAL_MESH";

#[derive(Debug, Clone, Copy)]
enum Shape {
    /// A 2D bounding box in the anchor's XY plane (`XrRect2Df`).
    Plane { offset: [f32; 2], extent: [f32; 2] },
    /// A 3D bounding box in the anchor's frame (`XrRect3DfFB`).
    Volume { offset: [f32; 3], extent: [f32; 3] },
}

struct Anchor {
    space: sys::Space,
    /// The anchor's UUID: stable across sessions and rescans (#3335), the
    /// key of its behavior in the room file.
    uuid: [u8; 16],
    label: String,
    shape: Shape,
    /// Last located pose in the base space, once tracking reports one.
    pose: Option<sys::Posef>,
    /// The LOCATABLE component is enabled (loaded anchors arrive with it
    /// off; `xrSetSpaceComponentStatusFB` turns it on asynchronously).
    locatable: bool,
}

/// The room's scene anchors as obstacle boxes.
pub struct Room {
    instance: xr::Instance,
    session: sys::Session,
    base: sys::Space,
    request: Option<sys::AsyncRequestIdFB>,
    /// Pending `xrRequestSceneCaptureFB`, if one was issued.
    capture: Option<sys::AsyncRequestIdFB>,
    /// Space Setup is requested at most once per run (a second empty result
    /// means something else is wrong, and a popup loop is worse than no room).
    capture_used: bool,
    /// Anchors the last query returned, before any were dropped.
    returned: usize,
    /// Empty results so far; the runtime answers with nothing until it has
    /// relocalized in the room, so the query is retried before Space Setup
    /// is considered.
    empty_results: u32,
    retry_at: Option<Instant>,
    /// Launch Space Setup when the query returns no anchors.
    allow_capture: bool,
    /// `debug.fosfora.rescan`: once the first query has returned anchors,
    /// run [`Self::rescan`] (`1`) or only [`Self::requery`] (`query`), so
    /// the replace path can be exercised over adb.
    rescan_at_start: Rescan,
    started: bool,
    anchors: Vec<Anchor>,
    /// The room id of `anchors` (`room_file::room_id`), recomputed when a
    /// query replaces them.
    id: Option<u64>,
    last_locate: Option<Instant>,
    /// Current obstacle boxes in the base space; rebuilt on relocate.
    pub boxes: Vec<ObstacleBox>,
    /// Each box's semantic labels, in `boxes` order; rebuilt with them.
    pub labels: std::sync::Arc<[String]>,
    /// Triangle count of the room's global mesh, if the runtime reports one
    /// (evidence only; the mesh is not an obstacle in the spike).
    pub mesh_triangles: Option<u32>,
}

impl Room {
    /// Prepare the scene query (`start` issues it once the session is
    /// focused). Needs `XR_FB_scene`, `XR_FB_spatial_entity` and
    /// `XR_FB_spatial_entity_query` enabled on the instance, and the
    /// `com.oculus.permission.USE_SCENE` runtime permission granted. With
    /// `allow_capture`, an empty result launches Space Setup
    /// (`XR_FB_scene_capture`) and the query reruns when it completes.
    /// With `rescan_at_start`, the first query that returns anchors is
    /// followed by a [`Self::rescan`] or a [`Self::requery`].
    pub fn new(
        session: &xr::Session<xr::Vulkan>,
        base: &xr::Space,
        allow_capture: bool,
        rescan_at_start: Rescan,
    ) -> Result<Self> {
        let instance = session.instance().clone();
        let exts = instance.exts();
        if exts.fb_scene.is_none()
            || exts.fb_spatial_entity.is_none()
            || exts.fb_spatial_entity_query.is_none()
        {
            return Err(anyhow!(
                "scene extensions not enabled (XR_FB_scene {}, XR_FB_spatial_entity {}, XR_FB_spatial_entity_query {})",
                exts.fb_scene.is_some(),
                exts.fb_spatial_entity.is_some(),
                exts.fb_spatial_entity_query.is_some()
            ));
        }
        let room = Self {
            instance,
            session: session.as_raw(),
            base: base.as_raw(),
            request: None,
            capture: None,
            capture_used: false,
            returned: 0,
            empty_results: 0,
            retry_at: None,
            allow_capture,
            rescan_at_start,
            started: false,
            anchors: Vec::new(),
            id: None,
            last_locate: None,
            boxes: Vec::new(),
            labels: std::sync::Arc::default(),
            mesh_triangles: None,
        };
        Ok(room)
    }

    /// Issue the query (once). Called when the session is first focused:
    /// Meta's samples query from a running session.
    pub fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        self.query_any();
    }

    /// The LOCAL storage filter (Meta's sample) failed validation on v207
    /// (`ERROR_VALIDATION_FAILURE`, Sep 27); the unfiltered query works. Try
    /// the filter first, fall back to no filter.
    fn query_any(&mut self) {
        if let Err(e) = self.query(true) {
            warn!("scene: filtered query: {e:#}; retrying unfiltered");
            if let Err(e) = self.query(false) {
                warn!("scene: {e:#}");
            }
        }
    }

    fn query(&mut self, filter_local: bool) -> Result<()> {
        let exts = self.instance.exts();
        let fp = exts
            .fb_spatial_entity_query
            .as_ref()
            .expect("checked in Room::new");
        // Filter to locally stored anchors (the room from Space Setup), as
        // Meta's scene sample does, when the storage extension is enabled;
        // otherwise no filter.
        let storage_filter = sys::SpaceStorageLocationFilterInfoFB {
            ty: sys::SpaceStorageLocationFilterInfoFB::TYPE,
            next: ptr::null(),
            location: sys::SpaceStorageLocationFB::LOCAL,
        };
        let filter: *const sys::SpaceFilterInfoBaseHeaderFB =
            if filter_local && exts.fb_spatial_entity_storage.is_some() {
                ptr::from_ref(&storage_filter).cast()
            } else {
                ptr::null()
            };
        let info = sys::SpaceQueryInfoFB {
            ty: sys::SpaceQueryInfoFB::TYPE,
            next: ptr::null(),
            query_action: sys::SpaceQueryActionFB::LOAD,
            max_result_count: MAX_RESULTS,
            timeout: sys::Duration::NONE,
            filter,
            exclude_filter: ptr::null(),
        };
        let mut request = sys::AsyncRequestIdFB::from_raw(0);
        // SAFETY: `info` is a complete XrSpaceQueryInfoFB whose header the
        // runtime reads as XrSpaceQueryInfoBaseHeaderFB; the session handle
        // is live for as long as this Room (dropped before the session).
        let result = unsafe {
            (fp.query_spaces)(
                self.session,
                ptr::from_ref(&info).cast::<sys::SpaceQueryInfoBaseHeaderFB>(),
                &mut request,
            )
        };
        check(result).context("xrQuerySpacesFB")?;
        info!(
            "scene: query started (request {}, {})",
            request.into_raw(),
            if filter.is_null() {
                "no filter"
            } else {
                "LOCAL storage filter"
            }
        );
        self.request = Some(request);
        Ok(())
    }

    /// `XR_TYPE_EVENT_DATA_SPACE_QUERY_RESULTS_AVAILABLE_FB` arrived: pull
    /// the results and read each anchor's components.
    pub fn results_available(&mut self, request: sys::AsyncRequestIdFB) {
        if self.request != Some(request) {
            warn!("scene: results for unknown request {}", request.into_raw());
            return;
        }
        if let Err(e) = self.retrieve(request) {
            warn!("scene: retrieving results: {e:#}");
        }
    }

    /// `XR_TYPE_EVENT_DATA_SPACE_QUERY_COMPLETE_FB` arrived.
    pub fn query_complete(&mut self, request: sys::AsyncRequestIdFB, result: sys::Result) {
        if self.request == Some(request) {
            if self.returned == 0 && self.capture.is_none() {
                self.empty_results += 1;
                if self.empty_results < EMPTY_RESULTS_BEFORE_CAPTURE {
                    info!(
                        "scene: no anchors yet ({}/{}), retrying in {:?}",
                        self.empty_results, EMPTY_RESULTS_BEFORE_CAPTURE, QUERY_RETRY_EVERY
                    );
                    self.retry_at = Some(Instant::now() + QUERY_RETRY_EVERY);
                } else if self.allow_capture && !self.capture_used {
                    self.capture_used = true;
                    self.request_capture();
                } else {
                    warn!(
                        "scene: no anchors: run Space Setup on the headset (or set debug.fosfora.scenecapture 1 to launch it)"
                    );
                }
            } else {
                self.empty_results = 0;
            }
            let rescan_now = (self.returned > 0).then_some(self.rescan_at_start);
            info!(
                "scene: query complete ({result:?}), {} anchors ({} planes, {} volumes)",
                self.anchors.len(),
                self.anchors
                    .iter()
                    .filter(|a| matches!(a.shape, Shape::Plane { .. }))
                    .count(),
                self.anchors
                    .iter()
                    .filter(|a| matches!(a.shape, Shape::Volume { .. }))
                    .count()
            );
            self.request = None;
            match rescan_now {
                Some(Rescan::Capture) => {
                    self.rescan_at_start = Rescan::Off;
                    info!("scene: debug.fosfora.rescan: rescanning the room now");
                    self.rescan();
                }
                Some(Rescan::Query) => {
                    self.rescan_at_start = Rescan::Off;
                    info!("scene: debug.fosfora.rescan query: querying the room again");
                    self.requery();
                }
                Some(Rescan::Off) | None => {}
            }
        }
    }

    /// Query the anchors again without Space Setup (the anchors returned
    /// replace the current ones). Ignored while a query is in flight.
    pub fn requery(&mut self) {
        if self.request.is_some() {
            info!("scene: requery: a query is in flight, try again");
            return;
        }
        self.retry_at = None;
        self.query_any();
    }

    /// Relaunch Space Setup and requery the anchors (the hand menu's
    /// "Rescan the room"), so a changed room takes without a reinstall.
    /// The anchors the new query returns replace the old ones
    /// ([`Self::retrieve`]); an empty result keeps them. Ignored while
    /// Space Setup or a query is already in flight.
    pub fn rescan(&mut self) {
        if self.capture.is_some() {
            info!("scene: rescan: Space Setup is already running");
            return;
        }
        if self.request.is_some() {
            info!("scene: rescan: a query is in flight, try again");
            return;
        }
        self.retry_at = None;
        self.capture_used = true;
        self.request_capture();
    }

    /// `XR_TYPE_EVENT_DATA_SPACE_SET_STATUS_COMPLETE_FB` arrived for one
    /// anchor's component.
    pub fn set_status_complete(
        &mut self,
        space: sys::Space,
        component: sys::SpaceComponentTypeFB,
        enabled: bool,
        result: sys::Result,
    ) {
        if component != sys::SpaceComponentTypeFB::LOCATABLE {
            return;
        }
        if let Some(anchor) = self.anchors.iter_mut().find(|a| a.space == space) {
            if result == sys::Result::SUCCESS && enabled {
                anchor.locatable = true;
                self.last_locate = None;
                info!("scene: anchor {} is now locatable", anchor.label);
            } else {
                warn!(
                    "scene: anchor {}: enabling LOCATABLE failed ({result:?}, enabled {enabled})",
                    anchor.label
                );
            }
        }
    }

    /// Ask the runtime to run Space Setup (`xrRequestSceneCaptureFB`).
    fn request_capture(&mut self) {
        let Some(fp) = self.instance.exts().fb_scene_capture.as_ref() else {
            warn!(
                "scene: no anchors and XR_FB_scene_capture is not enabled; run Space Setup by hand"
            );
            return;
        };
        let info = sys::SceneCaptureRequestInfoFB {
            ty: sys::SceneCaptureRequestInfoFB::TYPE,
            next: ptr::null(),
            request_byte_count: 0,
            request: ptr::null(),
        };
        let mut request = sys::AsyncRequestIdFB::from_raw(0);
        // SAFETY: `info` is a complete request struct (an empty request
        // means the default capture flow) and the session handle is live.
        let res = unsafe { (fp.request_scene_capture)(self.session, &info, &mut request) };
        match check(res) {
            Ok(()) => {
                info!(
                    "scene: Space Setup requested (request {})",
                    request.into_raw()
                );
                self.capture = Some(request);
            }
            Err(e) => warn!("scene: xrRequestSceneCaptureFB: {e:#}"),
        }
    }

    /// `XR_TYPE_EVENT_DATA_SCENE_CAPTURE_COMPLETE_FB` arrived: rerun the
    /// query so the new room becomes obstacles.
    pub fn capture_complete(&mut self, request: sys::AsyncRequestIdFB, result: sys::Result) {
        if self.capture != Some(request) {
            return;
        }
        info!("scene: Space Setup finished ({result:?}); querying again");
        self.capture = None;
        self.query_any();
    }

    fn retrieve(&mut self, request: sys::AsyncRequestIdFB) -> Result<()> {
        let exts = self.instance.exts();
        let query = exts
            .fb_spatial_entity_query
            .as_ref()
            .expect("checked in Room::new");
        let entity = exts
            .fb_spatial_entity
            .as_ref()
            .expect("checked in Room::new");
        let scene = exts.fb_scene.as_ref().expect("checked in Room::new");

        let mut results = sys::SpaceQueryResultsFB {
            ty: sys::SpaceQueryResultsFB::TYPE,
            next: ptr::null_mut(),
            result_capacity_input: 0,
            result_count_output: 0,
            results: ptr::null_mut(),
        };
        // SAFETY: two-call idiom; with capacity 0 the runtime only writes
        // `result_count_output`.
        check(unsafe { (query.retrieve_space_query_results)(self.session, request, &mut results) })
            .context("xrRetrieveSpaceQueryResultsFB (count)")?;
        let count = results.result_count_output;
        let mut buffer = vec![
            sys::SpaceQueryResultFB {
                space: sys::Space::NULL,
                uuid: sys::UuidEXT { data: [0; 16] },
            };
            count as usize
        ];
        results.result_capacity_input = count;
        results.results = buffer.as_mut_ptr();
        // SAFETY: `buffer` holds `count` elements and outlives the call.
        check(unsafe { (query.retrieve_space_query_results)(self.session, request, &mut results) })
            .context("xrRetrieveSpaceQueryResultsFB")?;
        buffer.truncate(results.result_count_output as usize);
        info!("scene: {} anchors returned", buffer.len());
        self.returned = buffer.len();
        // A new room replaces the old one; an empty answer (the runtime
        // not yet relocalized) keeps it and the retry below runs. The
        // runtime hands back the same XrSpace for an anchor it returned
        // before (a requery on v207, Sep 29), so the old anchors are
        // reconciled by handle: one the new results still carry is rebuilt
        // from them with its pose kept, one they no longer carry is
        // destroyed. Destroying them all first made the new results'
        // handles invalid and emptied the room.
        let old = if buffer.is_empty() {
            Vec::new()
        } else {
            std::mem::take(&mut self.anchors)
        };
        if !old.is_empty() {
            let kept = old
                .iter()
                .filter(|a| buffer.iter().any(|r| r.space == a.space))
                .count();
            info!(
                "scene: replacing {} anchors with {} results ({kept} kept, {} dropped)",
                old.len(),
                buffer.len(),
                old.len() - kept
            );
            self.boxes.clear();
            self.labels = std::sync::Arc::default();
            self.mesh_triangles = None;
        }
        let dropped: Vec<sys::Space> = old
            .iter()
            .filter(|a| !buffer.iter().any(|r| r.space == a.space))
            .map(|a| a.space)
            .collect();
        let old_pose =
            |space: sys::Space| old.iter().find(|a| a.space == space).and_then(|a| a.pose);

        for r in buffer {
            let space = r.space;
            let uuid = r.uuid.data;
            let enabled = |component: sys::SpaceComponentTypeFB| -> bool {
                let mut status = sys::SpaceComponentStatusFB {
                    ty: sys::SpaceComponentStatusFB::TYPE,
                    next: ptr::null_mut(),
                    enabled: sys::FALSE,
                    change_pending: sys::FALSE,
                };
                // SAFETY: `space` came from the runtime this call; `status`
                // is a valid out-struct.
                let res =
                    unsafe { (entity.get_space_component_status)(space, component, &mut status) };
                res == sys::Result::SUCCESS && status.enabled == sys::TRUE
            };
            let label = if enabled(sys::SpaceComponentTypeFB::SEMANTIC_LABELS) {
                semantic_labels(scene, self.session, space).unwrap_or_else(|e| {
                    warn!("scene: labels: {e:#}");
                    String::from("?")
                })
            } else {
                String::from("(unlabeled)")
            };
            if label.contains("GLOBAL_MESH") {
                if let Some(mesh) = exts.meta_spatial_entity_mesh.as_ref() {
                    match mesh_triangle_count(mesh, space) {
                        Ok(n) => {
                            info!(
                                "scene: global mesh with {n} triangles (not an obstacle in the spike)"
                            );
                            self.mesh_triangles = Some(n);
                        }
                        Err(e) => warn!("scene: mesh: {e:#}"),
                    }
                }
                destroy_space(&self.instance, space);
                continue;
            }
            let shape = if enabled(sys::SpaceComponentTypeFB::BOUNDED_3D) {
                let mut rect = sys::Rect3DfFB {
                    offset: sys::Offset3DfFB {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    extent: sys::Extent3DfFB {
                        width: 0.0,
                        height: 0.0,
                        depth: 0.0,
                    },
                };
                // SAFETY: valid out-struct; BOUNDED_3D is enabled on `space`.
                let res =
                    unsafe { (scene.get_space_bounding_box3_d)(self.session, space, &mut rect) };
                if check(res).is_err() {
                    warn!("scene: anchor {label}: xrGetSpaceBoundingBox3DFB {res:?}");
                    destroy_space(&self.instance, space);
                    continue;
                }
                Shape::Volume {
                    offset: [rect.offset.x, rect.offset.y, rect.offset.z],
                    extent: [rect.extent.width, rect.extent.height, rect.extent.depth],
                }
            } else if enabled(sys::SpaceComponentTypeFB::BOUNDED_2D) {
                let mut rect = sys::Rect2Df {
                    offset: sys::Offset2Df { x: 0.0, y: 0.0 },
                    extent: sys::Extent2Df {
                        width: 0.0,
                        height: 0.0,
                    },
                };
                // SAFETY: valid out-struct; BOUNDED_2D is enabled on `space`.
                let res =
                    unsafe { (scene.get_space_bounding_box2_d)(self.session, space, &mut rect) };
                if check(res).is_err() {
                    warn!("scene: anchor {label}: xrGetSpaceBoundingBox2DFB {res:?}");
                    destroy_space(&self.instance, space);
                    continue;
                }
                Shape::Plane {
                    offset: [rect.offset.x, rect.offset.y],
                    extent: [rect.extent.width, rect.extent.height],
                }
            } else {
                info!("scene: anchor {label}: no bounded component, skipped");
                destroy_space(&self.instance, space);
                continue;
            };
            // Loaded anchors come back with LOCATABLE off (all 18 did on
            // v207); enabling it is asynchronous and completes with
            // SPACE_SET_STATUS_COMPLETE_FB.
            let mut locatable = enabled(sys::SpaceComponentTypeFB::LOCATABLE);
            if !locatable {
                let set = sys::SpaceComponentStatusSetInfoFB {
                    ty: sys::SpaceComponentStatusSetInfoFB::TYPE,
                    next: ptr::null(),
                    component_type: sys::SpaceComponentTypeFB::LOCATABLE,
                    enabled: sys::TRUE,
                    timeout: sys::Duration::NONE,
                };
                let mut req = sys::AsyncRequestIdFB::from_raw(0);
                // SAFETY: `space` is a live anchor handle from this query and
                // `set` a complete request struct.
                let res = unsafe { (entity.set_space_component_status)(space, &set, &mut req) };
                match res {
                    sys::Result::SUCCESS => {}
                    // Already on and merely reported stale, or a change in flight.
                    sys::Result::ERROR_SPACE_COMPONENT_STATUS_ALREADY_SET_FB => locatable = true,
                    sys::Result::ERROR_SPACE_COMPONENT_STATUS_PENDING_FB => {}
                    other => warn!("scene: anchor {label}: xrSetSpaceComponentStatusFB {other:?}"),
                }
            }
            info!(
                "scene: anchor {label} {}: {shape:?}, locatable {locatable}",
                crate::room_file::uuid_hex(&uuid)
            );
            self.anchors.push(Anchor {
                space,
                uuid,
                label,
                shape,
                pose: old_pose(space),
                locatable,
            });
        }
        for space in dropped {
            destroy_space(&self.instance, space);
        }
        let id = crate::room_file::room_id(self.anchors.iter().map(|a| a.uuid));
        if id != self.id {
            info!(
                "scene: room id {} ({} anchors)",
                id.map_or("none".to_owned(), crate::room_file::room_id_hex),
                self.anchors.len()
            );
            self.id = id;
        }
        // Locate right away so the first frame after the query has boxes.
        self.last_locate = None;
        Ok(())
    }

    /// Relocate the anchors (rate limited) and rebuild `boxes`; also fires
    /// a pending query retry.
    pub fn locate(&mut self, time: xr::Time) {
        if self.retry_at.is_some_and(|t| Instant::now() >= t) {
            self.retry_at = None;
            self.query_any();
        }
        if self.anchors.is_empty() {
            return;
        }
        let now = Instant::now();
        if self
            .last_locate
            .is_some_and(|t| now.duration_since(t) < RELOCATE_EVERY)
        {
            return;
        }
        self.last_locate = Some(now);
        let locate = self.instance.fp().locate_space;
        let mut located = 0;
        for anchor in self.anchors.iter_mut().filter(|a| a.locatable) {
            let mut location = sys::SpaceLocation {
                ty: sys::SpaceLocation::TYPE,
                next: ptr::null_mut(),
                location_flags: sys::SpaceLocationFlags::EMPTY,
                pose: sys::Posef::IDENTITY,
            };
            // SAFETY: both spaces are live handles of this session; `location`
            // is a valid out-struct.
            let res = unsafe { locate(anchor.space, self.base, time, &mut location) };
            let valid = sys::SpaceLocationFlags::POSITION_VALID
                | sys::SpaceLocationFlags::ORIENTATION_VALID;
            if res == sys::Result::SUCCESS && location.location_flags.contains(valid) {
                if anchor.pose.is_none() {
                    let p = location.pose.position;
                    let o = location.pose.orientation;
                    // The anchor's local +Z in the base space: for planes the
                    // normal, for volumes the top-face normal (should be
                    // world up for floors and table tops).
                    let up = glam::Quat::from_xyzw(o.x, o.y, o.z, o.w) * glam::Vec3::Z;
                    info!(
                        "scene: anchor {} {} located at ({:.2}, {:.2}, {:.2}), local +Z -> ({:.2}, {:.2}, {:.2})",
                        anchor.label,
                        crate::room_file::uuid_hex(&anchor.uuid),
                        p.x,
                        p.y,
                        p.z,
                        up.x,
                        up.y,
                        up.z
                    );
                }
                anchor.pose = Some(location.pose);
                located += 1;
            }
        }
        self.boxes = self
            .anchors
            .iter()
            .filter_map(|a| a.pose.map(|pose| to_box(pose, a.shape, &a.label, a.uuid)))
            .collect();
        self.labels = self
            .anchors
            .iter()
            .filter(|a| a.pose.is_some())
            .map(|a| a.label.clone())
            .collect();
        if located != self.anchors.len() {
            info!(
                "scene: {located}/{} anchors located this pass",
                self.anchors.len()
            );
        }
    }

    /// The room id of the anchors the last query returned (every one, not
    /// only the located: the id holds while their LOCATABLE components
    /// come on one by one); `None` without anchors.
    pub fn room_id(&self) -> Option<u64> {
        self.id
    }

    pub fn anchor_summary(&self) -> String {
        self.anchors
            .iter()
            .map(|a| a.label.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        destroy_anchors(&self.instance, &mut self.anchors);
    }
}

/// Drop every anchor, its space destroyed.
fn destroy_anchors(instance: &xr::Instance, anchors: &mut Vec<Anchor>) {
    for anchor in anchors.drain(..) {
        destroy_space(instance, anchor.space);
    }
}

fn destroy_space(instance: &xr::Instance, space: sys::Space) {
    if space == sys::Space::NULL {
        return;
    }
    // SAFETY: `space` is a live anchor handle the runtime returned to us and
    // nothing else references it.
    let res = unsafe { (instance.fp().destroy_space)(space) };
    if res != sys::Result::SUCCESS {
        warn!("xrDestroySpace(anchor): {res:?}");
    }
}

/// An `XrResult` as a `Result`: success codes (including qualified ones
/// such as `SESSION_LOSS_PENDING`) pass, error codes carry their name.
pub(crate) fn check(result: sys::Result) -> Result<()> {
    if result.into_raw() >= 0 {
        Ok(())
    } else {
        Err(anyhow!("{result:?}"))
    }
}

fn semantic_labels(
    scene: &xr::raw::SceneFB,
    session: sys::Session,
    space: sys::Space,
) -> Result<String> {
    let support = sys::SemanticLabelsSupportInfoFB {
        ty: sys::SemanticLabelsSupportInfoFB::TYPE,
        next: ptr::null(),
        flags: sys::SemanticLabelsSupportFlagsFB::MULTIPLE_SEMANTIC_LABELS
            | sys::SemanticLabelsSupportFlagsFB::ACCEPT_DESK_TO_TABLE_MIGRATION
            | sys::SemanticLabelsSupportFlagsFB::ACCEPT_INVISIBLE_WALL_FACE,
        recognized_labels: RECOGNIZED_LABELS.as_ptr(),
    };
    let mut labels = sys::SemanticLabelsFB {
        ty: sys::SemanticLabelsFB::TYPE,
        next: ptr::from_ref(&support).cast(),
        buffer_capacity_input: 0,
        buffer_count_output: 0,
        buffer: ptr::null_mut(),
    };
    // SAFETY: two-call idiom, capacity 0 writes only the count; `support`
    // outlives both calls.
    check(unsafe { (scene.get_space_semantic_labels)(session, space, &mut labels) })
        .context("xrGetSpaceSemanticLabelsFB (count)")?;
    let mut buffer = vec![0 as c_char; labels.buffer_count_output as usize + 1];
    labels.buffer_capacity_input = labels.buffer_count_output;
    labels.buffer = buffer.as_mut_ptr();
    // SAFETY: `buffer` has `buffer_capacity_input` bytes and outlives the call.
    check(unsafe { (scene.get_space_semantic_labels)(session, space, &mut labels) })
        .context("xrGetSpaceSemanticLabelsFB")?;
    // SAFETY: `buffer` was allocated one byte longer than the runtime wrote
    // and zero-filled, so it is NUL-terminated within its bounds.
    let labels = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    Ok(labels.to_string_lossy().into_owned())
}

fn mesh_triangle_count(mesh: &xr::raw::SpatialEntityMeshMETA, space: sys::Space) -> Result<u32> {
    let info = sys::SpaceTriangleMeshGetInfoMETA {
        ty: sys::SpaceTriangleMeshGetInfoMETA::TYPE,
        next: ptr::null(),
    };
    let mut out = sys::SpaceTriangleMeshMETA {
        ty: sys::SpaceTriangleMeshMETA::TYPE,
        next: ptr::null_mut(),
        vertex_capacity_input: 0,
        vertex_count_output: 0,
        vertices: ptr::null_mut(),
        index_capacity_input: 0,
        index_count_output: 0,
        indices: ptr::null_mut(),
    };
    // SAFETY: capacity 0 on both arrays: the runtime writes only the counts.
    check(unsafe { (mesh.get_space_triangle_mesh)(space, &info, &mut out) })
        .context("xrGetSpaceTriangleMeshMETA (count)")?;
    Ok(out.index_count_output / 3)
}

/// An anchor's bounding shape in the base space as an oriented box of the
/// surface kind its `label` names, flagged as an emitter (its weight is
/// its behavior's) and as hidden for a wall the runtime hides, carrying
/// the anchor's `uuid`.
fn to_box(pose: sys::Posef, shape: Shape, label: &str, uuid: [u8; 16]) -> ObstacleBox {
    let q = glam::Quat::from_xyzw(
        pose.orientation.x,
        pose.orientation.y,
        pose.orientation.z,
        pose.orientation.w,
    );
    let origin = glam::Vec3::new(pose.position.x, pose.position.y, pose.position.z);
    let (local_center, half) = match shape {
        Shape::Plane { offset, extent } => (
            glam::Vec3::new(
                offset[0] + extent[0] * 0.5,
                offset[1] + extent[1] * 0.5,
                0.0,
            ),
            glam::Vec3::new(extent[0] * 0.5, extent[1] * 0.5, PLANE_HALF_THICKNESS_M),
        ),
        Shape::Volume { offset, extent } => (
            glam::Vec3::new(
                offset[0] + extent[0] * 0.5,
                offset[1] + extent[1] * 0.5,
                offset[2] + extent[2] * 0.5,
            ),
            glam::Vec3::new(extent[0] * 0.5, extent[1] * 0.5, extent[2] * 0.5),
        ),
    };
    let center = origin + q * local_center;
    ObstacleBox {
        center: center.to_array(),
        rot: q.to_array(),
        half: half.to_array(),
        kind: crate::surfaces::surface_kind(label),
        emit: 1.0,
        hidden: crate::surfaces::is_hidden_wall(label),
        uuid,
    }
}
