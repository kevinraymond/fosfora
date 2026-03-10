#!/usr/bin/env python3
"""
Phosphor Bridge — Azure Kinect Body Tracking (Placeholder)

This bridge defines the full schema for Azure Kinect skeleton tracking
(32 joints) but currently pushes zeros. It will produce live data once
the Azure Kinect Body Tracking SDK is set up.

Requirements:
    pip install websocket-client
    + Azure Kinect SDK + Body Tracking SDK
      (see learn.microsoft.com/azure/kinect-dk)

Run:
    python kinect_body.py
    python kinect_body.py --host 192.168.1.100

Fields produced:
    detected                — 0/1 body presence
    pelvis_x/y/z            — 32 joints x 3 axes = 96 fields
    spine_navel_x/y/z
    spine_chest_x/y/z
    neck_x/y/z
    ...
    body_count              — number of tracked bodies (normalized)
"""

import time
import sys
from phosphor_bridge import PhosphorBridge


# Azure Kinect Body Tracking joint names (32 joints)
JOINTS = [
    "pelvis",
    "spine_navel",
    "spine_chest",
    "neck",
    "clavicle_left",
    "shoulder_left",
    "elbow_left",
    "wrist_left",
    "hand_left",
    "handtip_left",
    "thumb_left",
    "clavicle_right",
    "shoulder_right",
    "elbow_right",
    "wrist_right",
    "hand_right",
    "handtip_right",
    "thumb_right",
    "hip_left",
    "knee_left",
    "ankle_left",
    "foot_left",
    "hip_right",
    "knee_right",
    "ankle_right",
    "foot_right",
    "head",
    "nose",
    "eye_left",
    "eye_right",
    "ear_left",
    "ear_right",
]

_HAS_KINECT = False
try:
    import pykinect_azure  # noqa: F401
    _HAS_KINECT = True
except ImportError:
    pass


def build_schema():
    fields = {}

    fields["detected"] = {
        "label": "Body Detected", "is_trigger": True,
    }

    fields["body_count"] = {
        "min": 0, "max": 1,
        "label": "Body Count (normalized)",
    }

    # 32 joints x 3 axes
    for joint in JOINTS:
        for axis in "xyz":
            fields[f"{joint}_{axis}"] = {
                "label": f"{joint.replace('_', ' ').title()} {axis.upper()}",
            }

    return fields


def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — Azure Kinect Body Tracking")
    args = parser.parse_args()

    if not _HAS_KINECT:
        print("=" * 60)
        print("  WARNING: Azure Kinect SDK not found.")
        print("  This bridge will connect and send schema but push zeros.")
        print("  Install Azure Kinect SDK + pykinect-azure for live data.")
        print("=" * 60)

    bridge = PhosphorBridge("kinect-body", args.host, args.port)
    schema = build_schema()
    bridge.declare_fields(schema)
    print(f"[kinect-body] Fields: {len(schema)}")

    if not bridge.connect():
        return

    dt = 1.0 / args.fps
    zeros = {k: 0.0 for k in schema}

    print(f"[kinect-body] Streaming at {args.fps} Hz. Ctrl+C to stop.")
    if not _HAS_KINECT:
        print("[kinect-body] Sending zeros (Azure Kinect SDK not available)")

    try:
        while True:
            t0 = time.time()

            if _HAS_KINECT:
                # TODO: Read from Kinect when SDK is available
                # tracker = pykinect_azure.start_body_tracker()
                # body = tracker.get_body()
                # ... extract joint data ...
                pass

            bridge.push(zeros)

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        bridge.shutdown()


if __name__ == "__main__":
    main()
