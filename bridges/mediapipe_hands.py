#!/usr/bin/env python3
"""
Phosphor Bridge — MediaPipe Hand Tracking

Tracks up to 2 hands via webcam and streams landmark positions plus
derived gesture floats (pinch, grab, spread, palm height) to Phosphor.

Install:
    pip install mediapipe opencv-python websocket-client

Run:
    python mediapipe_hands.py
    python mediapipe_hands.py --device 1 --fps 60 --show
    python mediapipe_hands.py --host 192.168.1.100    # remote Phosphor

Fields produced (per hand h1/h2):
    h1_detected             — 0/1 hand presence
    h1_wrist_x/y/z          — 21 landmarks x 3 axes = 63 fields
    h1_thumb_tip_x/y/z
    ...
    h1_pinch_distance       — thumb-to-index distance (0=touching)
    h1_grab_strength        — finger curl (0=open, 1=fist)
    h1_palm_y               — palm height in frame (0=top, 1=bottom)
    h1_spread               — index-to-pinky spread
    h1_velocity             — frame-to-frame wrist movement speed
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


# ── MediaPipe landmark names (21 per hand) ────────────────────────────

LANDMARKS = [
    "wrist",
    "thumb_cmc", "thumb_mcp", "thumb_ip", "thumb_tip",
    "index_mcp", "index_pip", "index_dip", "index_tip",
    "middle_mcp", "middle_pip", "middle_dip", "middle_tip",
    "ring_mcp", "ring_pip", "ring_dip", "ring_tip",
    "pinky_mcp", "pinky_pip", "pinky_dip", "pinky_tip",
]

HANDS = ["h1", "h2"]


# ── Helpers ───────────────────────────────────────────────────────────

def dist(a, b):
    return math.sqrt((a.x - b.x)**2 + (a.y - b.y)**2 + (a.z - b.z)**2)


def clamp01(v):
    return max(0.0, min(1.0, v))


def derive_gestures(lm):
    """Compute high-level gesture floats from 21 raw landmarks."""
    thumb_tip = lm[4]
    index_tip = lm[8]
    middle_tip = lm[12]
    ring_tip = lm[16]
    pinky_tip = lm[20]
    wrist = lm[0]

    # Pinch: thumb tip <-> index tip distance, normalized
    pinch = clamp01(dist(thumb_tip, index_tip) * 5.0)

    # Grab: average curl — how close fingertips are to wrist
    tips = [index_tip, middle_tip, ring_tip, pinky_tip]
    avg_dist = sum(dist(t, wrist) for t in tips) / 4.0
    grab = 1.0 - clamp01(avg_dist * 3.0)

    # Palm Y: wrist vertical position in frame
    palm_y = clamp01(wrist.y)

    # Spread: index tip <-> pinky tip distance
    spread = clamp01(dist(index_tip, pinky_tip) * 4.0)

    return {
        "pinch_distance": pinch,
        "grab_strength": grab,
        "palm_y": palm_y,
        "spread": spread,
    }


# ── Schema ────────────────────────────────────────────────────────────

def build_schema():
    fields = {}

    for h in HANDS:
        n = h[-1]  # "1" or "2"

        fields[f"{h}_detected"] = {
            "label": f"Hand {n} Detected", "is_trigger": True,
        }

        # 21 landmarks x 3 axes
        for lm_name in LANDMARKS:
            for axis in "xyz":
                fields[f"{h}_{lm_name}_{axis}"] = {
                    "label": f"H{n} {lm_name} {axis.upper()}",
                }

        # Derived gestures
        fields[f"{h}_pinch_distance"] = {"label": f"H{n} Pinch Distance"}
        fields[f"{h}_grab_strength"] = {"label": f"H{n} Grab Strength"}
        fields[f"{h}_palm_y"] = {"label": f"H{n} Palm Y"}
        fields[f"{h}_spread"] = {"label": f"H{n} Finger Spread"}
        fields[f"{h}_velocity"] = {"label": f"H{n} Wrist Velocity"}

    return fields


# ── Main ──────────────────────────────────────────────────────────────

def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — MediaPipe Hand Tracking")
    parser.add_argument("--device", type=int, default=0,
                        help="Camera device index")
    parser.add_argument("--max-hands", type=int, default=2,
                        help="Max simultaneous hands")
    parser.add_argument("--show", action="store_true",
                        help="Show camera preview window")
    args = parser.parse_args()

    # MediaPipe setup
    mp_hands = mp.solutions.hands
    hands = mp_hands.Hands(
        static_image_mode=False,
        max_num_hands=args.max_hands,
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
    print(f"[mediapipe-hands] Camera {args.device}: {w}x{h}")

    # Bridge
    bridge = PhosphorBridge("mediapipe-hands", args.host, args.port)
    schema = build_schema()
    bridge.declare_fields(schema)
    print(f"[mediapipe-hands] Fields: {len(schema)}")

    if not bridge.connect():
        return

    bridge.configure_preview(args)
    dt = 1.0 / args.fps
    prev_wrists = [None, None]

    print(f"[mediapipe-hands] Streaming at {args.fps} Hz. Ctrl+C to stop.")

    try:
        while True:
            t0 = time.time()

            ret, frame = cap.read()
            if not ret:
                time.sleep(0.01)
                continue

            rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
            result = hands.process(rgb)

            data = {}

            # Prepare annotated frame for preview
            need_annotate = bridge._preview_enabled or args.show
            annotated = frame.copy() if need_annotate else None

            for i, prefix in enumerate(HANDS):
                has_hand = (result.multi_hand_landmarks and
                            i < len(result.multi_hand_landmarks))

                if has_hand:
                    hand = result.multi_hand_landmarks[i]
                    data[f"{prefix}_detected"] = 1.0

                    # Raw landmarks
                    for j, lm_name in enumerate(LANDMARKS):
                        lm = hand.landmark[j]
                        data[f"{prefix}_{lm_name}_x"] = clamp01(lm.x)
                        data[f"{prefix}_{lm_name}_y"] = clamp01(lm.y)
                        data[f"{prefix}_{lm_name}_z"] = clamp01(lm.z + 0.5)

                    # Derived gestures
                    gestures = derive_gestures(hand.landmark)
                    for key, val in gestures.items():
                        data[f"{prefix}_{key}"] = val

                    # Velocity (frame-to-frame wrist displacement)
                    wrist = hand.landmark[0]
                    if prev_wrists[i] is not None:
                        pw = prev_wrists[i]
                        vel = math.sqrt(
                            (wrist.x - pw.x)**2 +
                            (wrist.y - pw.y)**2
                        ) * args.fps  # scale by fps for consistent units
                        data[f"{prefix}_velocity"] = clamp01(vel * 2.0)
                    else:
                        data[f"{prefix}_velocity"] = 0.0
                    prev_wrists[i] = wrist

                    # Draw landmarks on annotated frame
                    if need_annotate:
                        mp_draw.draw_landmarks(
                            annotated, hand, mp_hands.HAND_CONNECTIONS)
                else:
                    data[f"{prefix}_detected"] = 0.0
                    prev_wrists[i] = None

            bridge.push(data)

            if annotated is not None:
                bridge.push_preview(annotated)

            if args.show:
                cv2.imshow("Phosphor — MediaPipe Hands", annotated)
                if cv2.waitKey(1) & 0xFF == 27:
                    break

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        cap.release()
        hands.close()
        if args.show:
            cv2.destroyAllWindows()
        bridge.shutdown()


if __name__ == "__main__":
    main()
