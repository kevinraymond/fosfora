#!/usr/bin/env python3
"""
Phosphor Bridge Test Server — validates bridge output without Phosphor.

Listens on ws://localhost:9002/bind and prints incoming schema + data frames.
Use this to verify a bridge script is working before connecting to Phosphor.

Install:  pip install websockets
Run:      python test_echo_server.py
          (then in another terminal: python smart_lfo.py)
"""

import asyncio
import json
import os
import sys
import time

# Ensure print output is unbuffered (important when redirected to file/pipe)
os.environ.setdefault("PYTHONUNBUFFERED", "1")
sys.stdout.reconfigure(line_buffering=True)

try:
    import websockets
except ImportError:
    print("Install: pip install websockets")
    sys.exit(1)


frame_count = 0
last_print = 0
sources = {}


async def handler(ws):
    global frame_count, last_print, sources

    addr = ws.remote_address
    print(f"\n[server] Client connected from {addr}")

    try:
        async for message in ws:
            data = json.loads(message)
            msg_type = data.get("type", "?")
            source = data.get("source", "unknown")

            if msg_type == "schema":
                fields = data.get("fields", {})
                sources[source] = {
                    "fields": len(fields),
                    "frames": 0,
                    "connected_at": time.time(),
                }
                print(f"[{source}] Schema received: "
                      f"{len(fields)} fields")

                # Print first 10 field names
                for i, (fid, fdef) in enumerate(fields.items()):
                    if i >= 10:
                        print(f"  ... and {len(fields) - 10} more")
                        break
                    label = fdef.get("label", fid)
                    trigger = " [trigger]" if fdef.get("is_trigger") else ""
                    print(f"  {fid:30s}  {label}{trigger}")

            elif msg_type == "data":
                frame_count += 1
                if source in sources:
                    sources[source]["frames"] += 1

                fields = data.get("fields", {})

                # Print summary every second
                now = time.time()
                if now - last_print >= 1.0:
                    last_print = now

                    # Show a few live values
                    sample_fields = list(fields.items())[:5]
                    sample_str = "  ".join(
                        f"{k}={v:.3f}" for k, v in sample_fields
                    )

                    fps = frame_count  # frames in last ~1s
                    frame_count = 0

                    print(f"[{source}] {fps} fps, "
                          f"{len(fields)} fields | {sample_str}")

    except websockets.exceptions.ConnectionClosed:
        pass
    finally:
        print(f"[server] Client {addr} disconnected")
        if source in sources:
            s = sources[source]
            elapsed = time.time() - s["connected_at"]
            print(f"[{source}] Total: {s['frames']} frames "
                  f"in {elapsed:.1f}s")


async def main():
    import argparse
    parser = argparse.ArgumentParser(description="Phosphor Bridge Test Server")
    parser.add_argument("--port", type=int, default=9002,
                        help="Port to listen on (default: 9002)")
    args = parser.parse_args()

    port = args.port
    print(f"Phosphor Bridge Test Server")
    print(f"Listening on ws://localhost:{port}/bind")
    print(f"Waiting for bridge connections ...\n")

    async with websockets.serve(handler, "localhost", port):
        await asyncio.Future()  # run forever


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n[server] Shutdown")
