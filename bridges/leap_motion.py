#!/usr/bin/env python3
"""
Phosphor Bridge — Leap Motion Hand Tracker (Placeholder)

This bridge defines the full schema for Leap Motion hand tracking data
but currently pushes zeros. It will produce live data once the Leap SDK
Python bindings are available.

The Leap Motion Controller / Ultraleap provides sub-millimeter hand
tracking without a webcam — ideal for precise gesture control.

Requirements:
    pip install websocket-client
    + Leap Motion SDK (not yet available via pip — see ultraleap.com)

Run:
    python leap_motion.py
    python leap_motion.py --host 192.168.1.100

Fields produced (per hand h1/h2):
    h1_detected             — 0/1 hand presence
    h1_palm_x/y/z           — palm position (normalized to interaction box)
    h1_palm_yaw/pitch/roll  — palm orientation
    h1_grab_strength        — grab gesture (0=open, 1=fist)
    h1_pinch_strength       — pinch gesture (0=open, 1=pinching)
    h1_thumb_x/y/z          — fingertip positions (5 fingers x 3 axes)
    h1_index_x/y/z
    h1_middle_x/y/z
    h1_ring_x/y/z
    h1_pinky_x/y/z
"""

import time
import sys
from phosphor_bridge import PhosphorBridge


HANDS = ["h1", "h2"]
FINGERS = ["thumb", "index", "middle", "ring", "pinky"]

_HAS_LEAP = False
try:
    import Leap  # noqa: F401
    _HAS_LEAP = True
except ImportError:
    pass


def build_schema():
    fields = {}

    for h in HANDS:
        n = h[-1]

        fields[f"{h}_detected"] = {
            "label": f"Hand {n} Detected", "is_trigger": True,
        }

        # Palm position
        for axis in "xyz":
            fields[f"{h}_palm_{axis}"] = {
                "label": f"H{n} Palm {axis.upper()}",
            }

        # Palm orientation
        fields[f"{h}_palm_yaw"] = {"label": f"H{n} Palm Yaw"}
        fields[f"{h}_palm_pitch"] = {"label": f"H{n} Palm Pitch"}
        fields[f"{h}_palm_roll"] = {"label": f"H{n} Palm Roll"}

        # Gestures
        fields[f"{h}_grab_strength"] = {"label": f"H{n} Grab Strength"}
        fields[f"{h}_pinch_strength"] = {"label": f"H{n} Pinch Strength"}

        # Fingertip positions
        for finger in FINGERS:
            for axis in "xyz":
                fields[f"{h}_{finger}_{axis}"] = {
                    "label": f"H{n} {finger.title()} {axis.upper()}",
                }

    return fields


def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — Leap Motion Hand Tracker")
    args = parser.parse_args()

    if not _HAS_LEAP:
        print("=" * 60)
        print("  WARNING: Leap Motion SDK not found.")
        print("  This bridge will connect and send schema but push zeros.")
        print("  Install the Leap SDK from ultraleap.com for live data.")
        print("=" * 60)

    bridge = PhosphorBridge("leap-motion", args.host, args.port)
    schema = build_schema()
    bridge.declare_fields(schema)
    print(f"[leap-motion] Fields: {len(schema)}")

    if not bridge.connect():
        return

    dt = 1.0 / args.fps
    zeros = {k: 0.0 for k in schema}

    print(f"[leap-motion] Streaming at {args.fps} Hz. Ctrl+C to stop.")
    if not _HAS_LEAP:
        print("[leap-motion] Sending zeros (Leap SDK not available)")

    try:
        while True:
            t0 = time.time()

            if _HAS_LEAP:
                # TODO: Read from Leap controller when SDK is available
                # controller = Leap.Controller()
                # frame = controller.frame()
                # ... extract hand data ...
                pass

            bridge.push(zeros)

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        bridge.shutdown()


if __name__ == "__main__":
    main()
