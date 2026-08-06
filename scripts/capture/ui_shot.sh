#!/usr/bin/env bash
# Grab a still of the Fosfora interface — panels visible, music playing, meters moving.
#
# The README's interface screenshot previously existed only as a remote `user-attachments`
# URL with no copy in the repo, so it rendered as a broken image in the release tarball (which
# bundles README.md next to assets/). This produces a local one.
#
# Same isolation guarantees as capture.sh: stock config via XDG_CONFIG_HOME, audio routed by
# moving Fosfora's own capture stream to a private null sink, default sink untouched.
#
# Usage:  scripts/capture/ui_shot.sh [-o out.png] [--effect murmur] [--settle 25]

set -Eeuo pipefail

OUT=$PWD/ui.png
EFFECT=murmur
SETTLE=26
LISTEN=1

while [[ $# -gt 0 ]]; do
  case $1 in
    -o|--out)    OUT=$2; shift 2 ;;
    --effect)    EFFECT=$2; shift 2 ;;
    --settle)    SETTLE=$2; shift 2 ;;
    --no-listen) LISTEN=0; shift ;;
    -h|--help)   sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

REPO=$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
BIN=$REPO/target/release/fosfora
WORK=$(mktemp -d -t fosfora-uishot-XXXXXX)
CFG=$WORK/cfg
SINK=fosfora_uishot
APP_PID=  PLAY_PID=  SINK_MOD=  LOOPBACK_MOD=

log() { printf '\033[36m[ui-shot]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31m[ui-shot] %s\033[0m\n' "$*" >&2; exit 1; }
cleanup() {
  local rc=$?
  [[ -n $PLAY_PID ]] && kill "$PLAY_PID" 2>/dev/null || true
  [[ -n $APP_PID  ]] && { kill "$APP_PID" 2>/dev/null || true; sleep 0.5; kill -9 "$APP_PID" 2>/dev/null || true; }
  [[ -n $LOOPBACK_MOD ]] && pactl unload-module "$LOOPBACK_MOD" 2>/dev/null || true
  [[ -n $SINK_MOD     ]] && pactl unload-module "$SINK_MOD"     2>/dev/null || true
  rm -rf "$WORK"; exit $rc
}
trap cleanup EXIT INT TERM

[[ -x $BIN ]] || die "release binary not found — cargo build --release"
DEFAULT_SINK=$(pactl get-default-sink)
mkdir -p "$CFG/fosfora/splats" "$(dirname "$OUT")"

# Effect cycle order, as the app will scan it.
mapfile -t SLUGS < <(python3 - "$REPO/assets/effects" <<'PY'
import json, pathlib, sys
for p in sorted(pathlib.Path(sys.argv[1]).glob("*.pfx"), key=lambda p: p.name):
    if not json.loads(p.read_text()).get("hidden"):
        print(p.stem)
PY
)
TARGET=-1
for i in "${!SLUGS[@]}"; do [[ ${SLUGS[i]} == "$EFFECT" ]] && TARGET=$i; done
(( TARGET >= 0 )) || die "unknown effect '$EFFECT'"

"$REPO/scripts/capture/make_loop.py" -o "$WORK/loop.wav" >&2
SINK_MOD=$(pactl load-module module-null-sink sink_name=$SINK)
XDG_CONFIG_HOME=$CFG RUST_LOG=fosfora=info "$BIN" >"$WORK/app.log" 2>&1 &
APP_PID=$!

find_client_window() {
  local best= best_area=99999999 w area
  for w in $(xdotool search --name '^Fosfora$' 2>/dev/null); do
    eval "$(xdotool getwindowgeometry --shell "$w" 2>/dev/null)" || continue
    (( WIDTH < 200 || HEIGHT < 200 )) && continue
    area=$(( WIDTH * HEIGHT ))
    (( area < best_area )) && { best_area=$area; best=$w; }
  done
  [[ -n $best ]] && printf '%s' "$best"
}

WIN=
for _ in $(seq 60); do
  WIN=$(find_client_window) && [[ -n $WIN ]] && break
  kill -0 "$APP_PID" 2>/dev/null || die "app exited early; see $WORK/app.log"
  sleep 1
done
[[ -n $WIN ]] || die "window never appeared"
sleep 8

SO=$(pactl -f json list source-outputs | python3 -c "
import json,sys
for s in json.load(sys.stdin):
    if 'phosphor' in json.dumps(s.get('properties',{})).lower() or 'fosfora' in json.dumps(s.get('properties',{})).lower():
        print(s['index']); break")
[[ -n $SO ]] && pactl move-source-output "$SO" "$SINK.monitor"
(( LISTEN )) && LOOPBACK_MOD=$(pactl load-module module-loopback \
    source="$SINK.monitor" sink="$DEFAULT_SINK" latency_msec=60 2>/dev/null) || true

ffmpeg -hide_banner -loglevel error -re -stream_loop -1 -i "$WORK/loop.wav" \
       -f pulse -device "$SINK" fosfora-uishot &
PLAY_PID=$!

xdotool windowactivate --sync "$WIN"; sleep 0.5

# Boot lands on Phosphor (hidden); the first step goes to visible[1], so reaching index i
# takes i steps. The overlay stays VISIBLE here — it is the subject of the shot.
steps=$(( TARGET == 0 ? ${#SLUGS[@]} : TARGET ))
for ((i=0;i<steps;i++)); do
  oscsend localhost 9000 /fosfora/trigger/next_effect f 1.0; sleep 0.35
done
log "on $EFFECT; settling ${SETTLE}s so the meters have real history"
sleep "$SETTLE"

# Same motion-detection trick capture.sh uses, but only to place the grab; the panels are
# part of the client area so the client size is the right size to take.
FSX=999999 FSY=999999 FSR=0 FSB=0
for w in $(xdotool search --name '^Fosfora$'); do
  eval "$(xdotool getwindowgeometry --shell "$w" 2>/dev/null)" || continue
  (( X < FSX )) && FSX=$X; (( Y < FSY )) && FSY=$Y
  (( X + WIDTH  > FSR )) && FSR=$((X + WIDTH))
  (( Y + HEIGHT > FSB )) && FSB=$((Y + HEIGHT))
done
# --size is essential, not optional. Without it find_canvas.py falls back to a bounding box of
# all motion, and with the overlay VISIBLE the panels animate too — the box grew past the window
# and the shot caught a strip of the terminal underneath, including its text.
eval "$(xdotool getwindowgeometry --shell "$WIN")"
read -r X Y W H < <("$REPO/scripts/capture/find_canvas.py" \
                      --region "$FSX" "$FSY" "$((FSR-FSX))" "$((FSB-FSY))" \
                      --size "$WIDTH" "$HEIGHT" --display "$DISPLAY")
(( W > WIDTH  )) && W=$WIDTH
(( H > HEIGHT )) && H=$HEIGHT

ffmpeg -hide_banner -loglevel error -y -f x11grab -video_size "${W}x${H}" \
       -i "$DISPLAY+$X,$Y" -frames:v 1 "$OUT" </dev/null
log "wrote $OUT (${W}x${H})"
