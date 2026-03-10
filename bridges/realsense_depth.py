#!/usr/bin/env python3
"""
Phosphor Bridge — Intel RealSense Depth Camera

Streams depth-derived floats: closest object distance, center of mass,
depth histogram zones, motion amount, and presence detection.

Install:
    pip install pyrealsense2 numpy websocket-client

Run:
    python realsense_depth.py
    python realsense_depth.py --width 640 --height 480 --fps 30

Fields produced:
    closest_distance        — nearest valid depth (0=near, 1=far)
    closest_x/y             — position of closest point (normalized)
    com_x/y/depth           — inverse-depth-weighted center of mass
    zone_near/mid/far       — fraction of pixels in each depth zone
    motion_amount           — frame-to-frame depth change
    presence                — anything within 1.5m (0/1)
"""

import time
import sys

try:
    import numpy as np
except ImportError:
    print("Install: pip install numpy")
    sys.exit(1)

try:
    import pyrealsense2 as rs
except ImportError:
    print("Install: pip install pyrealsense2")
    sys.exit(1)

from phosphor_bridge import PhosphorBridge

# Depth zones (meters)
ZONE_NEAR = 0.5    # 0 - 0.5m
ZONE_MID = 2.0     # 0.5 - 2.0m
ZONE_FAR = 5.0     # 2.0 - 5.0m


def build_schema():
    return {
        # Closest object
        "closest_distance": {
            "min": 0, "max": 1,
            "label": "Closest Distance (0=near, 1=far)",
        },
        "closest_x": {
            "min": 0, "max": 1,
            "label": "Closest Object X",
        },
        "closest_y": {
            "min": 0, "max": 1,
            "label": "Closest Object Y",
        },

        # Center of mass of all depth
        "com_x": {"min": 0, "max": 1, "label": "Center of Mass X"},
        "com_y": {"min": 0, "max": 1, "label": "Center of Mass Y"},
        "com_depth": {"min": 0, "max": 1, "label": "Center of Mass Depth"},

        # Depth zone occupancy (fraction of pixels in each zone)
        "zone_near": {
            "min": 0, "max": 1,
            "label": f"Near Zone (<{ZONE_NEAR}m)",
        },
        "zone_mid": {
            "min": 0, "max": 1,
            "label": f"Mid Zone ({ZONE_NEAR}-{ZONE_MID}m)",
        },
        "zone_far": {
            "min": 0, "max": 1,
            "label": f"Far Zone ({ZONE_MID}-{ZONE_FAR}m)",
        },

        # Motion (frame-to-frame delta)
        "motion_amount": {
            "min": 0, "max": 1,
            "label": "Depth Motion Amount",
        },

        # Presence (anything within 1.5m)
        "presence": {
            "min": 0, "max": 1,
            "label": "Presence Detected",
            "is_trigger": True,
        },
    }


def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — RealSense Depth Camera")
    parser.add_argument("--width", type=int, default=640,
                        help="Depth stream width")
    parser.add_argument("--height", type=int, default=480,
                        help="Depth stream height")
    args = parser.parse_args()

    # Init RealSense
    pipeline = rs.pipeline()
    config = rs.config()
    config.enable_stream(rs.stream.depth, args.width, args.height,
                         rs.format.z16, args.fps)
    profile = pipeline.start(config)

    depth_sensor = profile.get_device().first_depth_sensor()
    depth_scale = depth_sensor.get_depth_scale()
    print(f"[realsense-depth] Depth scale: {depth_scale:.6f}")
    print(f"[realsense-depth] Resolution: {args.width}x{args.height} "
          f"@ {args.fps}fps")

    # Init bridge
    bridge = PhosphorBridge("realsense-depth", args.host, args.port)
    bridge.declare_fields(build_schema())

    if not bridge.connect():
        return

    prev_depth = None
    dt = 1.0 / args.fps
    print(f"[realsense-depth] Streaming. Ctrl+C to stop.")

    try:
        while True:
            t0 = time.time()

            frames = pipeline.wait_for_frames(timeout_ms=1000)
            depth_frame = frames.get_depth_frame()
            if not depth_frame:
                continue

            depth = np.asanyarray(depth_frame.get_data()).astype(np.float32)
            depth_m = depth * depth_scale  # convert to meters

            h, w = depth_m.shape
            valid = depth_m > 0.1  # ignore zero/noise

            data = {}

            if np.any(valid):
                valid_depths = depth_m[valid]

                # Closest object
                min_depth = np.min(valid_depths)
                min_pos = np.unravel_index(
                    np.argmin(np.where(valid, depth_m, 999)),
                    depth_m.shape
                )
                data["closest_distance"] = min(1.0, min_depth / ZONE_FAR)
                data["closest_x"] = min_pos[1] / w
                data["closest_y"] = min_pos[0] / h

                # Center of mass (weighted by inverse depth)
                weights = np.where(valid, 1.0 / (depth_m + 0.01), 0)
                total_weight = np.sum(weights)
                if total_weight > 0:
                    ys, xs = np.mgrid[0:h, 0:w]
                    data["com_x"] = float(
                        np.sum(xs * weights) / total_weight / w)
                    data["com_y"] = float(
                        np.sum(ys * weights) / total_weight / h)
                    data["com_depth"] = min(1.0, float(
                        np.sum(depth_m * weights) / total_weight / ZONE_FAR))
                else:
                    data["com_x"] = 0.5
                    data["com_y"] = 0.5
                    data["com_depth"] = 1.0

                # Depth zones
                total_valid = float(np.sum(valid))
                data["zone_near"] = float(np.sum(
                    valid & (depth_m < ZONE_NEAR)) / total_valid)
                data["zone_mid"] = float(np.sum(
                    valid & (depth_m >= ZONE_NEAR) & (depth_m < ZONE_MID))
                    / total_valid)
                data["zone_far"] = float(np.sum(
                    valid & (depth_m >= ZONE_MID) & (depth_m < ZONE_FAR))
                    / total_valid)

                # Presence
                near_pixels = np.sum(valid & (depth_m < 1.5))
                data["presence"] = 1.0 if near_pixels > (
                    total_valid * 0.02) else 0.0

                # Motion
                if prev_depth is not None:
                    delta = np.abs(depth_m - prev_depth)
                    data["motion_amount"] = min(1.0, float(
                        np.mean(delta[valid]) * 10.0))
                else:
                    data["motion_amount"] = 0.0

                prev_depth = depth_m.copy()

            else:
                data = {k: 0.0 for k in build_schema()}

            bridge.push(data)

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        pipeline.stop()
        bridge.shutdown()


if __name__ == "__main__":
    main()
