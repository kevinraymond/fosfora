#!/usr/bin/env python3
"""
Phosphor Bridge — MediaPipe Face Mesh (Curated Expressions)

Extracts ~16 expression floats from face landmark geometry rather than
streaming all 468 landmarks. This gives musically useful control signals
without overwhelming the binding matrix.

Install:
    pip install mediapipe opencv-python websocket-client

Run:
    python mediapipe_face.py
    python mediapipe_face.py --device 1 --show
    python mediapipe_face.py --host 192.168.1.100

Fields produced:
    detected                — 0/1 face presence
    mouth_open              — vertical mouth opening (0=closed, 1=wide)
    mouth_width             — horizontal mouth width (smile width)
    smile                   — smile detector (0=neutral, 1=big smile)
    eyebrow_raise_l/r       — eyebrow height above eye
    eye_open_l/r            — eye openness (0=closed, 1=wide)
    eye_squint_l/r          — eye squint amount
    head_yaw                — left-right head rotation (0=left, 1=right)
    head_pitch              — up-down head tilt (0=down, 1=up)
    head_roll               — head roll/tilt (0=left, 1=right)
    jaw_open                — jaw opening amount
    cheek_puff              — cheek puff approximation
    lip_pucker              — lip pucker approximation
    nose_wrinkle            — nose scrunch approximation
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


# ── Key landmark indices for expression computation ───────────────────
# MediaPipe Face Mesh uses 468 landmarks. We only need a curated subset.

# Mouth
UPPER_LIP_TOP = 13
LOWER_LIP_BOTTOM = 14
MOUTH_LEFT = 61
MOUTH_RIGHT = 291
UPPER_LIP_CENTER = 0
LOWER_LIP_CENTER = 17

# Eyes
LEFT_EYE_TOP = 159
LEFT_EYE_BOTTOM = 145
RIGHT_EYE_TOP = 386
RIGHT_EYE_BOTTOM = 374
LEFT_EYE_OUTER = 33
LEFT_EYE_INNER = 133
RIGHT_EYE_OUTER = 362
RIGHT_EYE_INNER = 263

# Eyebrows
LEFT_EYEBROW_TOP = 105
LEFT_EYE_REFERENCE = 159
RIGHT_EYEBROW_TOP = 334
RIGHT_EYE_REFERENCE = 386

# Head pose reference points
NOSE_TIP = 1
CHIN = 152
LEFT_CHEEK = 234
RIGHT_CHEEK = 454
FOREHEAD = 10

# Cheeks
LEFT_CHEEK_PUFF = 123
RIGHT_CHEEK_PUFF = 352
LEFT_CHEEK_BONE = 116
RIGHT_CHEEK_BONE = 345

# Nose
NOSE_BRIDGE = 6
NOSE_LEFT = 102
NOSE_RIGHT = 331


def clamp01(v):
    return max(0.0, min(1.0, v))


def dist(a, b):
    return math.sqrt((a.x - b.x)**2 + (a.y - b.y)**2)


def dist3d(a, b):
    return math.sqrt((a.x - b.x)**2 + (a.y - b.y)**2 + (a.z - b.z)**2)


def compute_expressions(lm):
    """Extract curated expression floats from 468 face landmarks."""

    # Mouth open: vertical distance between upper and lower lip
    mouth_h = dist(lm[UPPER_LIP_TOP], lm[LOWER_LIP_BOTTOM])
    mouth_open = clamp01(mouth_h * 15.0)

    # Mouth width: horizontal distance between mouth corners
    mouth_w = dist(lm[MOUTH_LEFT], lm[MOUTH_RIGHT])
    mouth_width = clamp01(mouth_w * 5.0)

    # Smile: mouth width relative to face width + slight upturn
    face_width = dist(lm[LEFT_CHEEK], lm[RIGHT_CHEEK])
    smile_ratio = mouth_w / (face_width + 0.001)
    # Smile also considers corners being above lip center
    corner_y = (lm[MOUTH_LEFT].y + lm[MOUTH_RIGHT].y) / 2.0
    lip_center_y = lm[LOWER_LIP_CENTER].y
    upturn = clamp01((lip_center_y - corner_y) * 20.0)
    smile = clamp01((smile_ratio - 0.25) * 4.0 + upturn * 0.3)

    # Eye openness
    left_eye_h = dist(lm[LEFT_EYE_TOP], lm[LEFT_EYE_BOTTOM])
    right_eye_h = dist(lm[RIGHT_EYE_TOP], lm[RIGHT_EYE_BOTTOM])
    eye_open_l = clamp01(left_eye_h * 30.0)
    eye_open_r = clamp01(right_eye_h * 30.0)

    # Eye squint (inverse of openness, more nuanced)
    eye_squint_l = clamp01(1.0 - eye_open_l * 1.5)
    eye_squint_r = clamp01(1.0 - eye_open_r * 1.5)

    # Eyebrow raise: distance from eyebrow to eye top
    left_brow_h = lm[LEFT_EYE_REFERENCE].y - lm[LEFT_EYEBROW_TOP].y
    right_brow_h = lm[RIGHT_EYE_REFERENCE].y - lm[RIGHT_EYEBROW_TOP].y
    eyebrow_raise_l = clamp01(left_brow_h * 25.0)
    eyebrow_raise_r = clamp01(right_brow_h * 25.0)

    # Head yaw: nose tip relative to cheek midpoints
    cheek_mid_x = (lm[LEFT_CHEEK].x + lm[RIGHT_CHEEK].x) / 2.0
    nose_offset = lm[NOSE_TIP].x - cheek_mid_x
    head_yaw = clamp01(nose_offset * 8.0 + 0.5)

    # Head pitch: nose tip vs forehead/chin midpoint
    face_center_y = (lm[FOREHEAD].y + lm[CHIN].y) / 2.0
    pitch_offset = lm[NOSE_TIP].y - face_center_y
    head_pitch = clamp01(-pitch_offset * 8.0 + 0.5)

    # Head roll: difference in eye heights
    eye_dy = lm[LEFT_EYE_OUTER].y - lm[RIGHT_EYE_OUTER].y
    head_roll = clamp01(eye_dy * 10.0 + 0.5)

    # Jaw open (similar to mouth open but uses chin distance)
    jaw_dist = dist(lm[NOSE_TIP], lm[CHIN])
    jaw_open = clamp01((jaw_dist * 8.0) - 0.5)

    # Cheek puff approximation: cheeks pushed outward
    left_puff = dist(lm[LEFT_CHEEK_PUFF], lm[NOSE_TIP])
    right_puff = dist(lm[RIGHT_CHEEK_PUFF], lm[NOSE_TIP])
    cheek_puff = clamp01(((left_puff + right_puff) * 4.0) - 0.5)

    # Lip pucker: mouth width narrows
    lip_pucker = clamp01(1.0 - mouth_width * 1.5)

    # Nose wrinkle: nose bridge compression
    nose_h = dist(lm[NOSE_BRIDGE], lm[NOSE_TIP])
    nose_wrinkle = clamp01(1.0 - nose_h * 15.0)

    return {
        "mouth_open": mouth_open,
        "mouth_width": mouth_width,
        "smile": smile,
        "eyebrow_raise_l": eyebrow_raise_l,
        "eyebrow_raise_r": eyebrow_raise_r,
        "eye_open_l": eye_open_l,
        "eye_open_r": eye_open_r,
        "eye_squint_l": eye_squint_l,
        "eye_squint_r": eye_squint_r,
        "head_yaw": head_yaw,
        "head_pitch": head_pitch,
        "head_roll": head_roll,
        "jaw_open": jaw_open,
        "cheek_puff": cheek_puff,
        "lip_pucker": lip_pucker,
        "nose_wrinkle": nose_wrinkle,
    }


def build_schema():
    fields = {}

    fields["detected"] = {
        "label": "Face Detected", "is_trigger": True,
    }

    expression_labels = {
        "mouth_open": "Mouth Open",
        "mouth_width": "Mouth Width",
        "smile": "Smile",
        "eyebrow_raise_l": "Left Eyebrow Raise",
        "eyebrow_raise_r": "Right Eyebrow Raise",
        "eye_open_l": "Left Eye Open",
        "eye_open_r": "Right Eye Open",
        "eye_squint_l": "Left Eye Squint",
        "eye_squint_r": "Right Eye Squint",
        "head_yaw": "Head Yaw (L-R)",
        "head_pitch": "Head Pitch (D-U)",
        "head_roll": "Head Roll (L-R)",
        "jaw_open": "Jaw Open",
        "cheek_puff": "Cheek Puff",
        "lip_pucker": "Lip Pucker",
        "nose_wrinkle": "Nose Wrinkle",
    }

    for fid, label in expression_labels.items():
        fields[fid] = {"label": label}

    return fields


def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — MediaPipe Face Mesh")
    parser.add_argument("--device", type=int, default=0,
                        help="Camera device index")
    parser.add_argument("--show", action="store_true",
                        help="Show camera preview window")
    args = parser.parse_args()

    # MediaPipe setup
    mp_face = mp.solutions.face_mesh
    face_mesh = mp_face.FaceMesh(
        static_image_mode=False,
        max_num_faces=1,
        refine_landmarks=True,
        min_detection_confidence=0.6,
        min_tracking_confidence=0.5,
    )
    mp_draw = mp.solutions.drawing_utils
    mp_draw_styles = mp.solutions.drawing_styles

    # Camera
    cap = cv2.VideoCapture(args.device)
    cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)
    w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    print(f"[mediapipe-face] Camera {args.device}: {w}x{h}")

    # Bridge
    bridge = PhosphorBridge("mediapipe-face", args.host, args.port)
    schema = build_schema()
    bridge.declare_fields(schema)
    print(f"[mediapipe-face] Fields: {len(schema)}")

    if not bridge.connect():
        return

    bridge.configure_preview(args)
    dt = 1.0 / args.fps
    print(f"[mediapipe-face] Streaming at {args.fps} Hz. Ctrl+C to stop.")

    try:
        while True:
            t0 = time.time()

            ret, frame = cap.read()
            if not ret:
                time.sleep(0.01)
                continue

            rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
            result = face_mesh.process(rgb)

            data = {}

            need_annotate = bridge._preview_enabled or args.show
            annotated = frame.copy() if need_annotate else None

            if result.multi_face_landmarks:
                face = result.multi_face_landmarks[0]
                data["detected"] = 1.0

                # Compute curated expressions
                expressions = compute_expressions(face.landmark)
                data.update(expressions)

                if need_annotate:
                    mp_draw.draw_landmarks(
                        annotated, face,
                        mp_face.FACEMESH_CONTOURS,
                        landmark_drawing_spec=None,
                        connection_drawing_spec=mp_draw_styles
                            .get_default_face_mesh_contours_style())
            else:
                data["detected"] = 0.0

            bridge.push(data)

            if annotated is not None:
                bridge.push_preview(annotated)

            if args.show:
                cv2.imshow("Phosphor — MediaPipe Face", annotated)
                if cv2.waitKey(1) & 0xFF == 27:
                    break

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        cap.release()
        face_mesh.close()
        if args.show:
            cv2.destroyAllWindows()
        bridge.shutdown()


if __name__ == "__main__":
    main()
