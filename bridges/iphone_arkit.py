#!/usr/bin/env python3
"""
Phosphor Bridge — iPhone ARKit Face Blend Shapes

Receives 52 ARKit blend shape values via UDP from an iPhone companion app
(e.g., Face Cap, Live Link Face, or a custom Swift app) and forwards them
to Phosphor's binding bus.

The iPhone app should send JSON packets over UDP to this script's port:
    {"blendShapes": {"eyeBlinkLeft": 0.8, "jawOpen": 0.3, ...}}

Install:
    pip install websocket-client

Run:
    python iphone_arkit.py
    python iphone_arkit.py --udp-port 5555 --host 192.168.1.100

Fields produced:
    52 ARKit blend shapes, all 0-1 normalized:
    eye_blink_left, eye_blink_right, jaw_open, mouth_smile_left, etc.
"""

import json
import socket
import time
import sys
from phosphor_bridge import PhosphorBridge


# ── ARKit Blend Shape names → Phosphor field IDs ─────────────────────
# ARKit uses camelCase; we convert to snake_case for Phosphor conventions

ARKIT_BLENDSHAPES = [
    "eyeBlinkLeft",
    "eyeLookDownLeft",
    "eyeLookInLeft",
    "eyeLookOutLeft",
    "eyeLookUpLeft",
    "eyeSquintLeft",
    "eyeWideLeft",
    "eyeBlinkRight",
    "eyeLookDownRight",
    "eyeLookInRight",
    "eyeLookOutRight",
    "eyeLookUpRight",
    "eyeSquintRight",
    "eyeWideRight",
    "jawForward",
    "jawLeft",
    "jawRight",
    "jawOpen",
    "mouthClose",
    "mouthFunnel",
    "mouthPucker",
    "mouthLeft",
    "mouthRight",
    "mouthSmileLeft",
    "mouthSmileRight",
    "mouthFrownLeft",
    "mouthFrownRight",
    "mouthDimpleLeft",
    "mouthDimpleRight",
    "mouthStretchLeft",
    "mouthStretchRight",
    "mouthRollLower",
    "mouthRollUpper",
    "mouthShrugLower",
    "mouthShrugUpper",
    "mouthPressLeft",
    "mouthPressRight",
    "mouthLowerDownLeft",
    "mouthLowerDownRight",
    "mouthUpperUpLeft",
    "mouthUpperUpRight",
    "browDownLeft",
    "browDownRight",
    "browInnerUp",
    "browOuterUpLeft",
    "browOuterUpRight",
    "cheekPuff",
    "cheekSquintLeft",
    "cheekSquintRight",
    "noseSneerLeft",
    "noseSneerRight",
    "tongueOut",
]


def camel_to_snake(name):
    """Convert camelCase to snake_case."""
    result = []
    for i, c in enumerate(name):
        if c.isupper() and i > 0:
            result.append("_")
        result.append(c.lower())
    return "".join(result)


# Build mapping: ARKit name -> Phosphor field ID
ARKIT_TO_FIELD = {
    name: camel_to_snake(name) for name in ARKIT_BLENDSHAPES
}

# Reverse mapping for faster lookups
FIELD_TO_ARKIT = {v: k for k, v in ARKIT_TO_FIELD.items()}


def build_schema():
    fields = {}

    fields["detected"] = {
        "label": "Face Detected", "is_trigger": True,
    }

    for arkit_name in ARKIT_BLENDSHAPES:
        field_id = ARKIT_TO_FIELD[arkit_name]
        # Make a human-readable label from the ARKit name
        label = camel_to_snake(arkit_name).replace("_", " ").title()
        fields[field_id] = {"label": label}

    return fields


def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — iPhone ARKit Face Blend Shapes")
    parser.add_argument("--udp-port", type=int, default=5555,
                        help="UDP port to listen for iPhone data")
    parser.add_argument("--udp-bind", default="0.0.0.0",
                        help="UDP bind address")
    args = parser.parse_args()

    # Bridge
    bridge = PhosphorBridge("iphone-arkit", args.host, args.port)
    schema = build_schema()
    bridge.declare_fields(schema)
    print(f"[iphone-arkit] Fields: {len(schema)}")

    if not bridge.connect():
        return

    # UDP receiver
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind((args.udp_bind, args.udp_port))
    sock.settimeout(0.1)  # 100ms timeout for graceful shutdown

    print(f"[iphone-arkit] Listening for UDP on "
          f"{args.udp_bind}:{args.udp_port}")
    print(f"[iphone-arkit] Waiting for iPhone data. Ctrl+C to stop.")

    last_data_time = 0

    try:
        while True:
            try:
                raw, addr = sock.recvfrom(4096)
            except socket.timeout:
                # If no data for 2 seconds, send detected=0
                if last_data_time > 0 and (
                        time.time() - last_data_time > 2.0):
                    bridge.push({"detected": 0.0})
                    last_data_time = 0
                continue

            try:
                packet = json.loads(raw.decode("utf-8"))
            except (json.JSONDecodeError, UnicodeDecodeError):
                continue

            # Extract blend shapes from packet
            blend_shapes = packet.get("blendShapes", packet.get("bs", {}))
            if not blend_shapes:
                continue

            data = {"detected": 1.0}

            for arkit_name, value in blend_shapes.items():
                field_id = ARKIT_TO_FIELD.get(arkit_name)
                if field_id and isinstance(value, (int, float)):
                    data[field_id] = max(0.0, min(1.0, float(value)))

            bridge.push(data)
            last_data_time = time.time()

    finally:
        sock.close()
        bridge.shutdown()


if __name__ == "__main__":
    main()
