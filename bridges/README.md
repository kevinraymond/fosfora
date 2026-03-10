# Phosphor Bridge Scripts

Companion scripts that stream external data sources into Phosphor's
binding bus via websocket. Run any bridge alongside Phosphor to add
new source types to the Binding Matrix.

## Quick Start

```bash
# 1. Start Phosphor (websocket server runs on port 9002 automatically)

# 2. Run a bridge:
pip install websocket-client
python bridges/smart_lfo.py

# 3. Open the Binding Matrix in Phosphor — new sources appear automatically
```

Or with Docker (no Python/pip needed):

```bash
docker run phosphor/bridge-smart-lfo
```

## Available Bridges

| Bridge | Source | Hardware | Install |
|--------|--------|----------|---------|
| `smart_lfo.py` | Generative oscillators | None | `pip install -r requirements-lfo.txt` |
| `mediapipe_hands.py` | Hand landmarks + gestures | Webcam | `pip install -r requirements-vision.txt` |
| `mediapipe_pose.py` | Body pose (33 landmarks) | Webcam | `pip install -r requirements-vision.txt` |
| `mediapipe_face.py` | Face expressions (16 floats) | Webcam | `pip install -r requirements-vision.txt` |
| `yolo_detect.py` | Object detection + tracking | Webcam | `pip install -r requirements-vision.txt` |
| `xbox_controller.py` | Gamepad sticks, triggers, buttons | Xbox controller | `pip install -r requirements-gamepad.txt` |
| `realsense_depth.py` | Depth zones + motion + presence | Intel RealSense | `pip install -r requirements-depth.txt` |
| `iphone_arkit.py` | 52 ARKit face blend shapes | iPhone (UDP) | `pip install -r requirements-lfo.txt` |
| `leap_motion.py` | Finger positions + gestures | Leap Motion | Leap SDK (placeholder) |
| `kinect_body.py` | 32-joint skeleton | Azure Kinect | Kinect SDK (placeholder) |

## Common Options

Every bridge accepts:

    --host HOST    Phosphor host (default: localhost)
    --port PORT    Websocket port (default: 9002)
    --fps FPS      Target frame rate (default: 30)

Most vision bridges also accept:

    --device N     Camera index (default: 0)
    --show         Show preview window

## Testing Without Phosphor

Use the echo server to verify bridge output:

```bash
# Terminal 1:
pip install websockets
python bridges/test_echo_server.py

# Terminal 2:
python bridges/smart_lfo.py
```

## Running on a Separate Machine

Bridges can run on a different computer on the same network:

```bash
python bridges/mediapipe_hands.py --host 192.168.1.100
```

This is useful for offloading ML inference to a dedicated GPU machine.

## Docker

### Single bridge

```bash
# Zero-hardware generative source
docker run phosphor/bridge-smart-lfo

# Xbox controller (needs /dev/input access)
docker run --privileged -v /dev/input:/dev/input:ro phosphor/bridge-gamepad

# Webcam hand tracking
docker run --device /dev/video0 phosphor/bridge-vision hands

# Other vision bridges (pose, face, yolo)
docker run --device /dev/video0 phosphor/bridge-vision pose
docker run --device /dev/video0 phosphor/bridge-vision face
docker run --device /dev/video0 phosphor/bridge-vision yolo
```

### Docker Compose

```bash
# Start hand tracking + LFO
docker compose -f bridges/docker-compose.yml up hands lfo

# Include depth camera
docker compose -f bridges/docker-compose.yml --profile depth up hands realsense lfo

# GPU-accelerated YOLO
docker compose -f bridges/docker-compose.yml --profile gpu up yolo-gpu hands lfo

# Point at Phosphor on another machine
PHOSPHOR_HOST=192.168.1.100 docker compose -f bridges/docker-compose.yml up hands lfo
```

### Building images

```bash
# Build from repo root
docker build -t phosphor/bridge-base -f bridges/docker/Dockerfile.base .
docker build -t phosphor/bridge-smart-lfo -f bridges/docker/Dockerfile.smart-lfo .
docker build -t phosphor/bridge-vision -f bridges/docker/Dockerfile.vision .
docker build -t phosphor/bridge-gamepad -f bridges/docker/Dockerfile.gamepad .
docker build -t phosphor/bridge-realsense -f bridges/docker/Dockerfile.realsense .
```

### Platform notes

- **Linux**: Everything works natively. Use `--network host` or explicit `--host` for Docker.
- **macOS**: Docker Desktop handles `host.docker.internal` automatically. For best MediaPipe performance, run scripts natively (uses CoreML/Metal).
- **Windows**: Docker Desktop with WSL2. NVIDIA GPU passthrough works via `nvidia-container-toolkit`.

## Writing Your Own Bridge

```python
from phosphor_bridge import PhosphorBridge
import time

bridge = PhosphorBridge("my-source")
bridge.declare_field("value_a", min=0, max=1, label="Value A")
bridge.declare_field("value_b", min=0, max=1, label="Value B")
bridge.connect()

while True:
    bridge.push({"value_a": 0.5, "value_b": 0.7})
    time.sleep(1/30)
```

Fields auto-appear in Phosphor's Binding Matrix as `ws.my-source.value_a`
as soon as the first data frame arrives.
