#!/usr/bin/env python3
"""
Phosphor Bridge Base — shared websocket client for all bridge scripts.

This is the foundation for all Phosphor companion bridges. It handles
websocket connection, schema declaration, data frame pushing, graceful
shutdown, and common CLI arguments.

Usage:
    from phosphor_bridge import PhosphorBridge

    bridge = PhosphorBridge("my-source")
    bridge.declare_field("sensor_x", min=0, max=1, label="Sensor X")
    bridge.connect()

    while True:
        bridge.push({"sensor_x": read_sensor()})
        time.sleep(1/30)

Protocol: see phosphor-binding-bus-addendum.md §2.5
"""

import json
import time
import signal
import sys
import argparse

try:
    import websocket
except ImportError:
    print("─────────────────────────────────────────────────")
    print("  Missing dependency: websocket-client")
    print("  Install:  pip install websocket-client")
    print("─────────────────────────────────────────────────")
    sys.exit(1)


class PhosphorBridge:

    def __init__(self, source_name, host="localhost", port=9002):
        """
        Args:
            source_name: Identifier for this source. Appears in Phosphor's
                         binding matrix as ws.{source_name}.{field_id}.
                         Use lowercase-hyphenated names.
            host: Phosphor websocket host.
            port: Phosphor websocket port.
        """
        self.source_name = source_name
        self.host = host
        self.port = port
        self.url = f"ws://{host}:{port}/bind"
        self.fields = {}
        self.ws = None
        self._connected = False
        self._shutdown_requested = False
        self._frame_count = 0
        self._start_time = None
        self._last_push_time = 0
        self._last_preview_time = 0
        self._preview_interval = 1.0 / 8  # default 8fps
        self._preview_enabled = True

        signal.signal(signal.SIGINT, self._handle_signal)
        signal.signal(signal.SIGTERM, self._handle_signal)

    # ── Schema ────────────────────────────────────────────────────────

    def declare_field(self, field_id, min=0.0, max=1.0, label=None,
                      is_trigger=False):
        """
        Register a field in the schema. Call before connect().

        Args:
            field_id: Short lowercase_underscored identifier.
            min: Expected minimum value (for UI display hints).
            max: Expected maximum value.
            label: Human-readable label shown in binding matrix.
            is_trigger: True for boolean/event fields (kick, detected, etc.)
        """
        self.fields[field_id] = {
            "min": min,
            "max": max,
            "label": label or field_id,
        }
        if is_trigger:
            self.fields[field_id]["is_trigger"] = True

    def declare_fields(self, field_defs):
        """
        Bulk register fields.

        Args:
            field_defs: dict of field_id -> {min, max, label, is_trigger}
        """
        for fid, fdef in field_defs.items():
            self.declare_field(fid, **fdef)

    # ── Connection ────────────────────────────────────────────────────

    def connect(self, retry_interval=2.0, max_retries=None):
        """
        Connect to Phosphor and send schema. Blocks until connected.

        Args:
            retry_interval: Seconds between retry attempts.
            max_retries: Max attempts (None = infinite until Ctrl+C).

        Returns:
            True if connected, False if shutdown requested.
        """
        attempts = 0

        while not self._shutdown_requested:
            try:
                self.ws = websocket.create_connection(self.url, timeout=5)
                # Short timeout so recv() in _drain() doesn't block
                self.ws.settimeout(0.0)
                self._connected = True

                _fields_str = (f"{len(self.fields)} fields"
                               if self.fields else "no schema")
                print(f"[{self.source_name}] Connected to {self.url}")

                if self.fields:
                    self.send_schema()

                self._start_time = time.time()
                self._frame_count = 0
                return True

            except Exception as e:
                attempts += 1
                if max_retries and attempts >= max_retries:
                    print(f"[{self.source_name}] Max retries reached.")
                    return False

                print(f"[{self.source_name}] "
                      f"Waiting for Phosphor at {self.url} ... "
                      f"({e.__class__.__name__})")
                time.sleep(retry_interval)

        return False

    def send_schema(self):
        """Send (or re-send) the current schema to Phosphor.

        Called automatically by connect(). Can also be called after
        declare_fields() to update the schema mid-session (e.g. when
        new dynamic fields are discovered).
        """
        if not self._connected or not self.ws:
            return
        schema = {
            "type": "schema",
            "source": self.source_name,
            "fields": self.fields,
        }
        self.ws.send(json.dumps(schema))
        print(f"[{self.source_name}] Schema sent: "
              f"{len(self.fields)} fields")

    def _reconnect(self):
        """Attempt to reconnect after a dropped connection."""
        self._connected = False
        print(f"[{self.source_name}] Connection lost. Reconnecting ...")
        return self.connect(retry_interval=1.0)

    # ── Data Push ─────────────────────────────────────────────────────

    def _drain(self):
        """Drain any inbound messages from Phosphor to prevent buffer backlog.

        Phosphor broadcasts state updates to all WS clients.  If the bridge
        never reads them the TCP receive buffer fills and Phosphor drops the
        connection (~5 000 frames / ~3 min).  Calling this on every push
        keeps the pipe clear at negligible cost.
        """
        try:
            while True:
                self.ws.recv()
        except (websocket.WebSocketException, BlockingIOError, OSError):
            pass

    def push(self, fields_dict):
        """
        Send a data frame to Phosphor.

        Args:
            fields_dict: dict mapping field_id -> float value.
                         Only include fields that changed, or all fields.
                         Phosphor holds last-known values for missing fields.
        """
        if not self._connected:
            return False

        # Drain inbound messages so Phosphor's send buffer never backs up
        self._drain()

        frame = {
            "type": "data",
            "source": self.source_name,
            "fields": fields_dict,
        }

        try:
            self.ws.send(json.dumps(frame))
            self._frame_count += 1
            self._last_push_time = time.time()
            return True
        except (websocket.WebSocketConnectionClosedException,
                BrokenPipeError, ConnectionResetError, OSError):
            return self._reconnect()

    def push_preview(self, frame):
        """
        Send an annotated camera frame as a thumbnail preview.

        Args:
            frame: OpenCV BGR image (numpy array).
        """
        if not self._preview_enabled or not self._connected:
            return False

        now = time.time()
        if now - self._last_preview_time < self._preview_interval:
            return False

        try:
            import cv2
            # Resize to 160x120 thumbnail
            thumb = cv2.resize(frame, (160, 120), interpolation=cv2.INTER_AREA)
            # JPEG encode at quality 50
            ok, jpeg = cv2.imencode('.jpg', thumb, [cv2.IMWRITE_JPEG_QUALITY, 50])
            if not ok:
                return False

            # Binary format: source_name_utf8 + 0x00 + jpeg_bytes
            data = self.source_name.encode('utf-8') + b'\x00' + jpeg.tobytes()
            self.ws.send(data, opcode=websocket.ABNF.OPCODE_BINARY)
            self._last_preview_time = now
            return True
        except (websocket.WebSocketConnectionClosedException,
                BrokenPipeError, ConnectionResetError, OSError):
            return False

    # ── Stats ─────────────────────────────────────────────────────────

    def stats(self):
        """Return runtime statistics dict."""
        elapsed = (time.time() - self._start_time
                   if self._start_time else 0)
        return {
            "source": self.source_name,
            "frames": self._frame_count,
            "elapsed_s": round(elapsed, 1),
            "avg_fps": (round(self._frame_count / elapsed, 1)
                        if elapsed > 0 else 0),
            "fields": len(self.fields),
            "connected": self._connected,
        }

    def print_stats(self):
        """Print a one-line stats summary."""
        s = self.stats()
        print(f"[{s['source']}] {s['frames']} frames, "
              f"{s['elapsed_s']}s, {s['avg_fps']} fps avg")

    # ── Shutdown ──────────────────────────────────────────────────────

    def shutdown(self):
        """Gracefully close the connection and print stats."""
        self._shutdown_requested = True

        if self.ws:
            try:
                self.ws.close()
            except Exception:
                pass

        self._connected = False
        s = self.stats()
        print(f"[{s['source']}] Shutdown — "
              f"{s['frames']} frames in {s['elapsed_s']}s "
              f"({s['avg_fps']} fps avg)")

    def _handle_signal(self, sig, frame):
        print()  # newline after ^C
        self.shutdown()
        sys.exit(0)

    # ── CLI Helpers ───────────────────────────────────────────────────

    @staticmethod
    def common_args(description="Phosphor Bridge"):
        """
        Create an ArgumentParser with standard bridge options.

        Returns:
            argparse.ArgumentParser with --host, --port, --fps defined.
        """
        parser = argparse.ArgumentParser(
            description=description,
            formatter_class=argparse.ArgumentDefaultsHelpFormatter,
        )
        parser.add_argument(
            "--host", default="localhost",
            help="Phosphor websocket host")
        parser.add_argument(
            "--port", type=int, default=9002,
            help="Phosphor websocket port")
        parser.add_argument(
            "--fps", type=int, default=30,
            help="Target push frame rate")
        parser.add_argument(
            "--no-preview", action="store_true",
            help="Disable thumbnail preview in Phosphor binding matrix")
        parser.add_argument(
            "--preview-fps", type=int, default=8,
            help="Preview thumbnail frame rate")
        return parser

    def configure_preview(self, args):
        """Configure preview from parsed CLI args."""
        self._preview_enabled = not getattr(args, 'no_preview', False)
        fps = getattr(args, 'preview_fps', 8)
        self._preview_interval = 1.0 / max(1, fps)
