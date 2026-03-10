#!/usr/bin/env python3
"""
Phosphor Bridge — MediaPipe Pose (Full Body)

Tracks 33 body landmarks via webcam and streams positions plus
derived body metrics to Phosphor's binding bus.

Install:
    pip install mediapipe opencv-python websocket-client

Run:
    python mediapipe_pose.py
    python mediapipe_pose.py --device 1 --show
    python mediapipe_pose.py --host 192.168.1.100

Fields produced:
    nose_x/y/z ... (33 landmarks x 3 axes = 99 fields)
    shoulder_width          — distance between shoulders (normalized)
    torso_lean              — lateral lean (0=left, 0.5=center, 1=right)
    arm_raise_l/r           — how high each arm is raised (0=down, 1=up)
    leg_spread              — distance between ankles (normalized)
    head_tilt               — head lateral tilt (0=left, 0.5=center, 1=right)
    body_height             — overall body height in frame
    detected                — 0/1 body presence
"""

import time
import math
import sys

try:
    import cv2
except ImportError:
    print("Install: pip install opencv-python")
    sys.exit(1)

try:
    import mediapipe as mp
except ImportError:
    print("Install: pip install mediapipe")
    sys.exit(1)

from phosphor_bridge import PhosphorBridge


# ── MediaPipe Pose landmark names (33 total) ─────────────────────────

LANDMARKS = [
    "nose",
    "left_eye_inner", "left_eye", "left_eye_outer",
    "right_eye_inner", "right_eye", "right_eye_outer",
    "left_ear", "right_ear",
    "mouth_left", "mouth_right",
    "left_shoulder", "right_shoulder",
    "left_elbow", "right_elbow",
    "left_wrist", "right_wrist",
    "left_pinky", "right_pinky",
    "left_index", "right_index",
    "left_thumb", "right_thumb",
    "left_hip", "right_hip",
    "left_knee", "right_knee",
    "left_ankle", "right_ankle",
    "left_heel", "right_heel",
    "left_foot_index", "right_foot_index",
]

# Landmark indices for derived metrics
L_SHOULDER = 11
R_SHOULDER = 12
L_ELBOW = 13
R_ELBOW = 14
L_WRIST = 15
R_WRIST = 16
L_HIP = 23
R_HIP = 24
L_ANKLE = 27
R_ANKLE = 28
NOSE = 0
L_EAR = 7
R_EAR = 8


def clamp01(v):
    return max(0.0, min(1.0, v))


def dist2d(a, b):
    return math.sqrt((a.x - b.x)**2 + (a.y - b.y)**2)


def build_schema():
    fields = {}

    fields["detected"] = {
        "label": "Body Detected", "is_trigger": True,
    }

    # 33 landmarks x 3 axes
    for lm_name in LANDMARKS:
        for axis in "xyz":
            fields[f"{lm_name}_{axis}"] = {
                "label": f"{lm_name.replace('_', ' ').title()} {axis.upper()}",
            }

    # Derived body metrics
    fields["shoulder_width"] = {"label": "Shoulder Width"}
    fields["torso_lean"] = {"label": "Torso Lean (L-C-R)"}
    fields["arm_raise_l"] = {"label": "Left Arm Raise"}
    fields["arm_raise_r"] = {"label": "Right Arm Raise"}
    fields["leg_spread"] = {"label": "Leg Spread"}
    fields["head_tilt"] = {"label": "Head Tilt (L-C-R)"}
    fields["body_height"] = {"label": "Body Height"}

    return fields


def compute_derived(lm):
    """Compute derived body metrics from 33 raw landmarks."""
    ls = lm[L_SHOULDER]
    rs = lm[R_SHOULDER]
    lh = lm[L_HIP]
    rh = lm[R_HIP]
    lw = lm[L_WRIST]
    rw = lm[R_WRIST]
    la = lm[L_ANKLE]
    ra = lm[R_ANKLE]
    nose = lm[NOSE]
    le = lm[L_EAR]
    re = lm[R_EAR]

    # Shoulder width (normalized by frame width, roughly 0-0.5)
    shoulder_width = clamp01(dist2d(ls, rs) * 3.0)

    # Torso lean: midpoint of shoulders vs midpoint of hips, X axis
    shoulder_mid_x = (ls.x + rs.x) / 2.0
    hip_mid_x = (lh.x + rh.x) / 2.0
    lean = clamp01((shoulder_mid_x - hip_mid_x) * 5.0 + 0.5)

    # Arm raise: how high wrist is relative to shoulder (inverted Y)
    arm_raise_l = clamp01((ls.y - lw.y) * 3.0)
    arm_raise_r = clamp01((rs.y - rw.y) * 3.0)

    # Leg spread: ankle-to-ankle distance
    leg_spread = clamp01(dist2d(la, ra) * 3.0)

    # Head tilt: difference in ear Y positions
    head_tilt = clamp01((le.y - re.y) * 5.0 + 0.5)

    # Body height: nose to ankle midpoint distance
    ankle_mid_y = (la.y + ra.y) / 2.0
    body_height = clamp01(abs(ankle_mid_y - nose.y) * 1.5)

    return {
        "shoulder_width": shoulder_width,
        "torso_lean": lean,
        "arm_raise_l": arm_raise_l,
        "arm_raise_r": arm_raise_r,
        "leg_spread": leg_spread,
        "head_tilt": head_tilt,
        "body_height": body_height,
    }


def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — MediaPipe Pose")
    parser.add_argument("--device", type=int, default=0,
                        help="Camera device index")
    parser.add_argument("--show", action="store_true",
                        help="Show camera preview window")
    args = parser.parse_args()

    # MediaPipe setup
    mp_pose = mp.solutions.pose
    pose = mp_pose.Pose(
        static_image_mode=False,
        model_complexity=1,
        min_detection_confidence=0.6,
        min_tracking_confidence=0.5,
    )
    mp_draw = mp.solutions.drawing_utils

    # Camera
    cap = cv2.VideoCapture(args.device)
    cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)
    w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    print(f"[mediapipe-pose] Camera {args.device}: {w}x{h}")

    # Bridge
    bridge = PhosphorBridge("mediapipe-pose", args.host, args.port)
    schema = build_schema()
    bridge.declare_fields(schema)
    print(f"[mediapipe-pose] Fields: {len(schema)}")

    if not bridge.connect():
        return

    bridge.configure_preview(args)
    dt = 1.0 / args.fps
    print(f"[mediapipe-pose] Streaming at {args.fps} Hz. Ctrl+C to stop.")

    try:
        while True:
            t0 = time.time()

            ret, frame = cap.read()
            if not ret:
                time.sleep(0.01)
                continue

            rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
            result = pose.process(rgb)

            data = {}

            need_annotate = bridge._preview_enabled or args.show
            annotated = frame.copy() if need_annotate else None

            if result.pose_landmarks:
                data["detected"] = 1.0
                lm = result.pose_landmarks.landmark

                # Raw landmarks
                for j, lm_name in enumerate(LANDMARKS):
                    pt = lm[j]
                    data[f"{lm_name}_x"] = clamp01(pt.x)
                    data[f"{lm_name}_y"] = clamp01(pt.y)
                    data[f"{lm_name}_z"] = clamp01(pt.z + 0.5)

                # Derived metrics
                derived = compute_derived(lm)
                data.update(derived)

                if need_annotate:
                    mp_draw.draw_landmarks(
                        annotated, result.pose_landmarks,
                        mp_pose.POSE_CONNECTIONS)
            else:
                data["detected"] = 0.0

            bridge.push(data)

            if annotated is not None:
                bridge.push_preview(annotated)

            if args.show:
                cv2.imshow("Phosphor — MediaPipe Pose", annotated)
                if cv2.waitKey(1) & 0xFF == 27:
                    break

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        cap.release()
        pose.close()
        if args.show:
            cv2.destroyAllWindows()
        bridge.shutdown()


if __name__ == "__main__":
    main()
