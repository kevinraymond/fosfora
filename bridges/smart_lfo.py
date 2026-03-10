#!/usr/bin/env python3
"""
Phosphor Bridge — Smart LFO Generator

Generates evolving oscillator control signals that create organic
movement without any hardware. Each LFO runs a different waveform
at a different rate, with slow drift to prevent repetitive loops.

Great for: ambient movement, generative backgrounds, demo/testing.

Install:
    pip install websocket-client

Run:
    python smart_lfo.py
    python smart_lfo.py --lfo-count 8 --base-rate 0.2
    python smart_lfo.py --host 192.168.1.100

Fields produced:
    lfo_0 through lfo_N     — individual oscillator values (0-1)
    lfo_sum                 — averaged sum of all LFOs
    lfo_max                 — current maximum across all LFOs
    lfo_spread              — difference between max and min LFOs
    lfo_energy              — overall energy (RMS of all LFOs)
"""

import time
import math
import sys
from phosphor_bridge import PhosphorBridge


# ── Waveforms ─────────────────────────────────────────────────────────

def sine(phase):
    return (math.sin(phase * math.tau) + 1.0) / 2.0


def triangle(phase):
    return 1.0 - abs(2.0 * (phase % 1.0) - 1.0)


def saw_up(phase):
    return phase % 1.0


def saw_down(phase):
    return 1.0 - (phase % 1.0)


def square(phase):
    return 1.0 if (phase % 1.0) < 0.5 else 0.0


def smooth_noise(phase):
    """Smooth pseudo-random oscillation using layered sines."""
    v = 0.0
    v += 0.50 * math.sin(phase * 1.0000)
    v += 0.25 * math.sin(phase * 2.7183 + 0.5)
    v += 0.15 * math.sin(phase * 4.1231 + 1.3)
    v += 0.10 * math.sin(phase * 7.3891 + 2.1)
    return max(0.0, min(1.0, (v + 0.5)))


def breath(phase):
    """Organic breathing rhythm — quick inhale, slow exhale."""
    p = phase % 1.0
    if p < 0.35:
        return (math.sin(p / 0.35 * math.pi / 2.0)) ** 0.7
    else:
        return (math.cos((p - 0.35) / 0.65 * math.pi / 2.0)) ** 1.3


WAVEFORMS = [
    ("sine", sine),
    ("triangle", triangle),
    ("saw_up", saw_up),
    ("noise", smooth_noise),
    ("breath", breath),
    ("saw_down", saw_down),
    ("square", square),
]


# ── Schema ────────────────────────────────────────────────────────────

def build_schema(count):
    fields = {}

    for i in range(count):
        wf_name = WAVEFORMS[i % len(WAVEFORMS)][0]
        fields[f"lfo_{i}"] = {
            "min": 0, "max": 1,
            "label": f"LFO {i} ({wf_name})",
        }

    fields["lfo_sum"] = {"min": 0, "max": 1, "label": "LFO Average"}
    fields["lfo_max"] = {"min": 0, "max": 1, "label": "LFO Max"}
    fields["lfo_spread"] = {"min": 0, "max": 1, "label": "LFO Spread"}
    fields["lfo_energy"] = {"min": 0, "max": 1, "label": "LFO Energy (RMS)"}

    return fields


# ── Main ──────────────────────────────────────────────────────────────

def main():
    parser = PhosphorBridge.common_args(
        "Phosphor Bridge — Smart LFO Generator")
    parser.add_argument("--lfo-count", type=int, default=6,
                        help="Number of LFO channels")
    parser.add_argument("--base-rate", type=float, default=0.3,
                        help="Base oscillation rate in Hz")
    parser.add_argument("--drift", type=float, default=0.05,
                        help="Slow rate drift amount")
    args = parser.parse_args()

    bridge = PhosphorBridge("smart-lfo", args.host, args.port)
    bridge.declare_fields(build_schema(args.lfo_count))

    if not bridge.connect():
        return

    # Build LFO configs — each has staggered rate and phase
    lfos = []
    for i in range(args.lfo_count):
        wf_name, wf_fn = WAVEFORMS[i % len(WAVEFORMS)]
        lfos.append({
            "rate": args.base_rate * (0.5 + i * 0.35),
            "phase_offset": i * 0.1337,
            "waveform": wf_fn,
            "drift_seed": i * 3.7,
        })

    dt = 1.0 / args.fps
    t = 0.0

    rates_str = ", ".join(f"{l['rate']:.2f}" for l in lfos)
    print(f"[smart-lfo] {args.lfo_count} LFOs @ rates [{rates_str}] Hz")
    print(f"[smart-lfo] Streaming at {args.fps} Hz. Ctrl+C to stop.")

    try:
        while True:
            t0 = time.time()
            t += dt

            data = {}
            values = []

            for i, lfo in enumerate(lfos):
                # Slowly drift the rate for non-repetitive motion
                rate = lfo["rate"] + args.drift * math.sin(
                    t * 0.07 + lfo["drift_seed"])

                phase = t * rate + lfo["phase_offset"]
                val = lfo["waveform"](phase)

                data[f"lfo_{i}"] = val
                values.append(val)

            # Composites
            data["lfo_sum"] = sum(values) / len(values)
            data["lfo_max"] = max(values)
            data["lfo_spread"] = max(values) - min(values)
            data["lfo_energy"] = math.sqrt(
                sum(v * v for v in values) / len(values))

            bridge.push(data)

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        bridge.shutdown()


if __name__ == "__main__":
    main()
