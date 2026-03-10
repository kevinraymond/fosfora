#!/usr/bin/env python3
"""
Phosphor Bridge — Xbox Controller

Streams gamepad inputs (sticks, triggers, buttons, d-pad) as bindable
parameters into Phosphor's binding bus. Works with Xbox One S/X, Xbox
Series, and compatible controllers via the Linux xpad/xone driver.

Install:
    pip install websocket-client evdev

Run:
    python xbox_controller.py
    python xbox_controller.py --deadzone 0.15
    python xbox_controller.py --device /dev/input/event24

Fields produced (23):
    left_x, left_y              — left stick  (0-1, center 0.5)
    right_x, right_y            — right stick (0-1, center 0.5)
    trigger_left, trigger_right — analog triggers (0-1)
    left_magnitude              — left stick distance from center
    right_magnitude             — right stick distance from center
    dpad_up/down/left/right     — d-pad (triggers)
    btn_a/b/x/y                 — face buttons (triggers)
    btn_lb/rb                   — bumpers (triggers)
    btn_start/back/xbox         — menu buttons (triggers)
    btn_lstick/btn_rstick       — stick clicks (triggers)
"""

import math
import os
import sys
import time
from phosphor_bridge import PhosphorBridge

try:
    import evdev
    from evdev import ecodes
except ImportError:
    print("─────────────────────────────────────────────────")
    print("  Missing dependency: evdev")
    print("  Install:  pip install evdev")
    print("─────────────────────────────────────────────────")
    sys.exit(1)


# ── Device Discovery ─────────────────────────────────────────────────

# Axes required to qualify as a gamepad (both sticks + triggers)
GAMEPAD_AXES = {ecodes.ABS_X, ecodes.ABS_Y, ecodes.ABS_RX, ecodes.ABS_RY}

def _is_gamepad(dev):
    """Check if a device has dual analog sticks (i.e. is a gamepad)."""
    caps = dev.capabilities(verbose=False)
    if ecodes.EV_ABS not in caps:
        return False
    abs_codes = {a[0] if isinstance(a, tuple) else a for a in caps[ecodes.EV_ABS]}
    return GAMEPAD_AXES.issubset(abs_codes)


def find_gamepad(device_path=None):
    """Find a gamepad, returning an evdev.InputDevice or None."""
    if device_path:
        try:
            dev = evdev.InputDevice(device_path)
            print(f"[gamepad] Using device: {dev.name} ({dev.path})")
            return dev
        except (FileNotFoundError, PermissionError) as e:
            if isinstance(e, PermissionError):
                print(f"[gamepad] Permission denied: {device_path}")
                print(f"[gamepad] Try: sudo usermod -aG input $USER  (then re-login)")
            else:
                print(f"[gamepad] Device not found: {device_path}")
            return None

    try:
        devices = [evdev.InputDevice(path) for path in evdev.list_devices()]
    except PermissionError:
        print("[gamepad] Permission denied listing /dev/input/")
        print("[gamepad] Try: sudo usermod -aG input $USER  (then re-login)")
        return None

    for dev in devices:
        if _is_gamepad(dev):
            print(f"[gamepad] Found: {dev.name} ({dev.path})")
            return dev

    return None


# ── Schema ───────────────────────────────────────────────────────────

def build_schema():
    fields = {}

    # Analog sticks
    for prefix, label in [("left", "Left Stick"), ("right", "Right Stick")]:
        fields[f"{prefix}_x"] = {"min": 0, "max": 1, "label": f"{label} X"}
        fields[f"{prefix}_y"] = {"min": 0, "max": 1, "label": f"{label} Y"}

    # Triggers
    fields["trigger_left"]  = {"min": 0, "max": 1, "label": "Left Trigger"}
    fields["trigger_right"] = {"min": 0, "max": 1, "label": "Right Trigger"}

    # Magnitudes
    fields["left_magnitude"]  = {"min": 0, "max": 1, "label": "Left Stick Magnitude"}
    fields["right_magnitude"] = {"min": 0, "max": 1, "label": "Right Stick Magnitude"}

    # D-pad
    for name, label in [("dpad_up", "D-Pad Up"), ("dpad_down", "D-Pad Down"),
                         ("dpad_left", "D-Pad Left"), ("dpad_right", "D-Pad Right")]:
        fields[name] = {"min": 0, "max": 1, "label": label, "is_trigger": True}

    # Buttons
    buttons = [
        ("btn_a", "A"), ("btn_b", "B"), ("btn_x", "X"), ("btn_y", "Y"),
        ("btn_lb", "LB"), ("btn_rb", "RB"),
        ("btn_start", "Start"), ("btn_back", "Back"), ("btn_xbox", "Xbox"),
        ("btn_lstick", "Left Stick Click"), ("btn_rstick", "Right Stick Click"),
    ]
    for name, label in buttons:
        fields[name] = {"min": 0, "max": 1, "label": label, "is_trigger": True}

    return fields


# ── Event Mapping ────────────────────────────────────────────────────

# Xbox controller axis ranges
STICK_MIN, STICK_MAX = -32768, 32767
TRIGGER_MIN, TRIGGER_MAX = 0, 1023

# Button code → field name
BUTTON_MAP = {
    ecodes.BTN_SOUTH:  "btn_a",
    ecodes.BTN_EAST:   "btn_b",
    ecodes.BTN_NORTH:  "btn_x",
    ecodes.BTN_WEST:   "btn_y",
    ecodes.BTN_TL:     "btn_lb",
    ecodes.BTN_TR:     "btn_rb",
    ecodes.BTN_START:  "btn_start",
    ecodes.BTN_SELECT: "btn_back",
    ecodes.BTN_MODE:   "btn_xbox",
    ecodes.BTN_THUMBL: "btn_lstick",
    ecodes.BTN_THUMBR: "btn_rstick",
}


def normalize_stick(value):
    """Map stick axis (-32768..32767) to 0..1."""
    return (value - STICK_MIN) / (STICK_MAX - STICK_MIN)


def normalize_trigger(value):
    """Map trigger axis (0..1023) to 0..1."""
    return value / TRIGGER_MAX


def apply_deadzone(x, y, deadzone):
    """Apply radial deadzone to centered stick values (0.5 center)."""
    dx = x - 0.5
    dy = y - 0.5
    dist = math.sqrt(dx * dx + dy * dy)
    if dist < deadzone:
        return 0.5, 0.5, 0.0
    # Rescale so edge of deadzone maps to 0 displacement
    scale = (dist - deadzone) / (0.5 - deadzone) / dist
    return 0.5 + dx * scale, 0.5 + dy * scale, min(1.0, (dist - deadzone) / (0.5 - deadzone))


# ── Main ─────────────────────────────────────────────────────────────

def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — Gamepad Controller")
    parser.add_argument("--device", default=None,
                        help="evdev device path (e.g. /dev/input/event24)")
    parser.add_argument("--deadzone", type=float, default=0.12,
                        help="Radial deadzone for analog sticks")
    args = parser.parse_args()
    args.fps = args.fps if args.fps != 30 else 60  # default 60 for gamepad

    bridge = PhosphorBridge("gamepad", args.host, args.port)
    bridge.declare_fields(build_schema())

    if not bridge.connect():
        return

    dt = 1.0 / args.fps

    # State
    state = {
        "left_x": 0.5, "left_y": 0.5,
        "right_x": 0.5, "right_y": 0.5,
        "trigger_left": 0.0, "trigger_right": 0.0,
        "left_magnitude": 0.0, "right_magnitude": 0.0,
        "dpad_up": 0.0, "dpad_down": 0.0,
        "dpad_left": 0.0, "dpad_right": 0.0,
    }
    for btn_field in BUTTON_MAP.values():
        state[btn_field] = 0.0

    # Raw stick values before deadzone (0..1 normalized)
    raw_lx, raw_ly = 0.5, 0.5
    raw_rx, raw_ry = 0.5, 0.5

    dev = None

    try:
        while True:
            # Device discovery / reconnection
            if dev is None:
                dev = find_gamepad(args.device)
                if dev is None:
                    print("[gamepad] No controller found. Retrying in 2s ...")
                    time.sleep(2.0)
                    continue
                print(f"[gamepad] Streaming at {args.fps} Hz. Ctrl+C to stop.")

            t0 = time.time()

            # Drain all pending events
            try:
                while True:
                    event = dev.read_one()
                    if event is None:
                        break

                    if event.type == ecodes.EV_ABS:
                        code, val = event.code, event.value

                        if code == ecodes.ABS_X:
                            raw_lx = normalize_stick(val)
                        elif code == ecodes.ABS_Y:
                            raw_ly = 1.0 - normalize_stick(val)  # invert Y
                        elif code == ecodes.ABS_RX:
                            raw_rx = normalize_stick(val)
                        elif code == ecodes.ABS_RY:
                            raw_ry = 1.0 - normalize_stick(val)  # invert Y
                        elif code == ecodes.ABS_Z:
                            state["trigger_left"] = normalize_trigger(val)
                        elif code == ecodes.ABS_RZ:
                            state["trigger_right"] = normalize_trigger(val)
                        elif code == ecodes.ABS_HAT0X:
                            state["dpad_left"]  = 1.0 if val == -1 else 0.0
                            state["dpad_right"] = 1.0 if val ==  1 else 0.0
                        elif code == ecodes.ABS_HAT0Y:
                            state["dpad_up"]   = 1.0 if val == -1 else 0.0
                            state["dpad_down"] = 1.0 if val ==  1 else 0.0

                    elif event.type == ecodes.EV_KEY:
                        field = BUTTON_MAP.get(event.code)
                        if field:
                            state[field] = 1.0 if event.value else 0.0

            except OSError:
                print("[gamepad] Controller disconnected.")
                dev = None
                continue

            # Apply deadzone and compute magnitudes
            lx, ly, lm = apply_deadzone(raw_lx, raw_ly, args.deadzone)
            rx, ry, rm = apply_deadzone(raw_rx, raw_ry, args.deadzone)
            state["left_x"] = lx
            state["left_y"] = ly
            state["left_magnitude"] = lm
            state["right_x"] = rx
            state["right_y"] = ry
            state["right_magnitude"] = rm

            bridge.push(state)

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        bridge.shutdown()


if __name__ == "__main__":
    main()
