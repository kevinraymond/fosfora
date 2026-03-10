#!/bin/bash
# Phosphor vision bridge entrypoint — selects bridge by name.

BRIDGE=${1:-hands}
HOST=${PHOSPHOR_HOST:-host.docker.internal}
PORT=${PHOSPHOR_PORT:-9002}
FPS=${PHOSPHOR_FPS:-30}
DEVICE=${PHOSPHOR_DEVICE:-0}

# Shift past the bridge name so remaining args pass through
shift 2>/dev/null || true

case "$BRIDGE" in
    hands)
        exec python mediapipe_hands.py --host "$HOST" --port "$PORT" \
            --fps "$FPS" --device "$DEVICE" "$@"
        ;;
    pose)
        exec python mediapipe_pose.py --host "$HOST" --port "$PORT" \
            --fps "$FPS" --device "$DEVICE" "$@"
        ;;
    face)
        exec python mediapipe_face.py --host "$HOST" --port "$PORT" \
            --fps "$FPS" --device "$DEVICE" "$@"
        ;;
    yolo)
        exec python yolo_detect.py --host "$HOST" --port "$PORT" \
            --fps "$FPS" --device "$DEVICE" "$@"
        ;;
    *)
        echo "Unknown bridge: $BRIDGE"
        echo "Available: hands, pose, face, yolo"
        exit 1
        ;;
esac
