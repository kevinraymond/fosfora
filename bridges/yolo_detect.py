#!/usr/bin/env python3
"""
Phosphor Bridge — YOLO Object Detection

Detects objects via webcam and streams bounding box data + counts
to Phosphor's binding bus. Uses ultralytics YOLOv8.

Install:
    pip install ultralytics opencv-python websocket-client

Run:
    python yolo_detect.py
    python yolo_detect.py --model yolov8s.pt --confidence 0.5
    python yolo_detect.py --show

Fields are produced dynamically — only classes actually detected are
sent to Phosphor. When a new class appears, the schema is re-sent.
Phosphor expires individual fields after 5s of no updates, so classes
that leave the frame are automatically cleaned up.

Per class, up to 3 instances are tracked:
    {class}_count           — number detected (normalized 0-1, max 10)
    {class}_0_cx/cy         — bounding box center (normalized)
    {class}_0_w/h           — bounding box size (normalized)
    {class}_0_conf          — detection confidence
    ...
    total_objects           — total detection count (normalized)
"""

import time
import sys

try:
    import cv2
except ImportError:
    print("Install: pip install opencv-python")
    sys.exit(1)

try:
    from ultralytics import YOLO
except ImportError:
    print("Install: pip install ultralytics")
    sys.exit(1)

from phosphor_bridge import PhosphorBridge

MAX_INSTANCES = 3  # track up to N instances per class


def build_schema(seen_classes, coco_names):
    """Build schema fields for all currently seen classes."""
    fields = {}

    for cls_id in sorted(seen_classes):
        cls_name = coco_names[cls_id]
        fields[f"{cls_name}_count"] = {
            "min": 0, "max": 10,
            "label": f"{cls_name.replace('_', ' ').title()} Count",
        }

        for i in range(MAX_INSTANCES):
            prefix = f"{cls_name}_{i}"
            label_base = f"{cls_name.replace('_', ' ').title()} {i}"
            fields[f"{prefix}_cx"] = {
                "min": 0, "max": 1,
                "label": f"{label_base} Center X",
            }
            fields[f"{prefix}_cy"] = {
                "min": 0, "max": 1,
                "label": f"{label_base} Center Y",
            }
            fields[f"{prefix}_w"] = {
                "min": 0, "max": 1,
                "label": f"{label_base} Width",
            }
            fields[f"{prefix}_h"] = {
                "min": 0, "max": 1,
                "label": f"{label_base} Height",
            }
            fields[f"{prefix}_conf"] = {
                "min": 0, "max": 1,
                "label": f"{label_base} Confidence",
            }

    fields["total_objects"] = {
        "min": 0, "max": 30,
        "label": "Total Objects",
    }

    return fields


def main():
    parser = PhosphorBridge.common_args("Phosphor Bridge — YOLO Detection")
    parser.add_argument("--device", type=int, default=0,
                        help="Camera device index")
    parser.add_argument("--model", default="yolov8n.pt",
                        help="YOLO model (yolov8n/s/m/l/x.pt)")
    parser.add_argument("--confidence", type=float, default=0.4,
                        help="Detection confidence threshold")
    parser.add_argument("--show", action="store_true",
                        help="Show detection preview")
    args = parser.parse_args()

    # Init YOLO
    model = YOLO(args.model)
    coco_names = {
        cid: name.lower().replace(" ", "_")
        for cid, name in model.names.items()
    }
    print(f"[yolo-detect] Model: {args.model} ({len(coco_names)} classes)")

    # Init camera
    cap = cv2.VideoCapture(args.device)
    w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    print(f"[yolo-detect] Camera {args.device}: {w}x{h}")

    # Init bridge (no schema yet — will be sent on first detection)
    bridge = PhosphorBridge("yolo-detect", args.host, args.port)

    if not bridge.connect():
        return

    bridge.configure_preview(args)
    seen_classes = set()
    dt = 1.0 / args.fps
    print(f"[yolo-detect] Streaming at {args.fps} Hz. Ctrl+C to stop.")

    try:
        while True:
            t0 = time.time()

            ret, frame = cap.read()
            if not ret:
                time.sleep(0.01)
                continue

            # Run inference
            results = model(frame, conf=args.confidence, verbose=False)[0]
            boxes = results.boxes

            # Group detections by class
            frame_classes = {}  # cls_id -> list of instances
            total = 0

            for box in boxes:
                cls_id = int(box.cls[0])
                x1, y1, x2, y2 = box.xyxyn[0].tolist()
                frame_classes.setdefault(cls_id, []).append({
                    "cx": (x1 + x2) / 2,
                    "cy": (y1 + y2) / 2,
                    "w": x2 - x1,
                    "h": y2 - y1,
                    "conf": float(box.conf[0]),
                })
                total += 1

            # Check for new classes — re-send schema if any appeared
            new_classes = set(frame_classes.keys()) - seen_classes
            if new_classes:
                for cid in sorted(new_classes):
                    print(f"[yolo-detect] New class: {coco_names[cid]}")
                seen_classes |= new_classes
                schema = build_schema(seen_classes, coco_names)
                bridge.declare_fields(schema)
                bridge.send_schema()

            # Only send data for classes detected this frame.
            # Phosphor expires individual fields after 5s of no updates,
            # so absent classes are automatically cleaned up.
            data = {}
            for cls_id, instances in frame_classes.items():
                cls_name = coco_names[cls_id]
                instances.sort(key=lambda x: x["conf"], reverse=True)

                data[f"{cls_name}_count"] = min(10, len(instances)) / 10.0

                for i in range(min(MAX_INSTANCES, len(instances))):
                    inst = instances[i]
                    prefix = f"{cls_name}_{i}"
                    data[f"{prefix}_cx"] = inst["cx"]
                    data[f"{prefix}_cy"] = inst["cy"]
                    data[f"{prefix}_w"] = inst["w"]
                    data[f"{prefix}_h"] = inst["h"]
                    data[f"{prefix}_conf"] = inst["conf"]

            data["total_objects"] = min(1.0, total / 30.0)

            bridge.push(data)

            if bridge._preview_enabled:
                annotated = results.plot()
                bridge.push_preview(annotated)

            if args.show:
                show_frame = annotated if bridge._preview_enabled else results.plot()
                cv2.imshow("Phosphor — YOLO Detect", show_frame)
                if cv2.waitKey(1) & 0xFF == 27:
                    break

            elapsed = time.time() - t0
            if elapsed < dt:
                time.sleep(dt - elapsed)

    finally:
        cap.release()
        if args.show:
            cv2.destroyAllWindows()
        bridge.shutdown()


if __name__ == "__main__":
    main()
