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
# The app opens on the workspace's Build view. --preset loads one of the demo presets in
# scripts/capture/demos (by its cue in _manifest.tsv) instead of stepping to one effect, so
# the stack shows several layers. The First run tour is marked done, or it would cover the
# shot. The grab is the window's own pixmap (xwd), so windows stacked over it do not matter.
#
# Usage:  scripts/capture/ui_shot.sh [-o out.png] [--effect murmur | --preset "Advanced 2 Stack"]
#                                    [--settle 25]

set -Eeuo pipefail

OUT=$PWD/ui.png
EFFECT=murmur
PRESET=
SETTLE=26
LISTEN=1

while [[ $# -gt 0 ]]; do
  case $1 in
    -o|--out)    OUT=$2; shift 2 ;;
    --effect)    EFFECT=$2; shift 2 ;;
    --preset)    PRESET=$2; shift 2 ;;
    --settle)    SETTLE=$2; shift 2 ;;
    --no-listen) LISTEN=0; shift ;;
    -h|--help)   sed -n '2,17p' "$0"; exit 0 ;;
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
# Another instance on the OSC port would take every oscsend and leave this one idle.
ss -lun | grep -q ':9000 ' && die "something already listens on UDP 9000 (a stray fosfora?)"
DEFAULT_SINK=$(pactl get-default-sink)
mkdir -p "$CFG/fosfora/splats" "$(dirname "$OUT")"
printf '{"version":1,"theme":"Gray","tours_done":["first_run"]}\n' > "$CFG/fosfora/settings.json"

DEMOS=$REPO/scripts/capture/demos
SCENE_NAME="Fosfora Advanced"
CUE=
if [[ -n $PRESET ]]; then
  CUE=$(awk -F'\t' -v p="$PRESET" '$3 == p { print $2 }' "$DEMOS/_manifest.tsv")
  [[ -n $CUE ]] || die "'$PRESET' is not a preset in $DEMOS/_manifest.tsv"
  mkdir -p "$CFG/fosfora/presets" "$CFG/fosfora/scenes"
  for f in "$DEMOS"/*.json; do
    b=$(basename "$f"); [[ $b == _* ]] && continue
    sed "s|@REPO@|$REPO|g; s|@WORK@|$WORK|g" "$f" > "$CFG/fosfora/presets/$b"
  done
  sed "s|@REPO@|$REPO|g; s|@WORK@|$WORK|g" "$DEMOS/_scene.json" \
    > "$CFG/fosfora/scenes/$SCENE_NAME.json"
fi

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

if [[ -n $PRESET ]]; then
  # The scene resolves the preset by name, so this does not depend on how many built-in
  # presets ship (see capture_advanced.sh).
  oscsend localhost 9000 /fosfora/scene/load s "$SCENE_NAME"; sleep 0.4
  oscsend localhost 9000 /fosfora/scene/goto_cue i "$CUE"; sleep 3
  grep -qF "Loaded preset '$PRESET'" "$WORK/app.log" || die "preset '$PRESET' never loaded"
  EFFECT=$PRESET
else
  # Boot lands on Fosfora (hidden); the first step goes to visible[1], so reaching index i
  # takes i steps. The overlay stays VISIBLE here — it is the subject of the shot.
  steps=$(( TARGET == 0 ? ${#SLUGS[@]} : TARGET ))
  for ((i=0;i<steps;i++)); do
    oscsend localhost 9000 /fosfora/trigger/next_effect f 1.0; sleep 0.35
  done
fi
log "on $EFFECT; settling ${SETTLE}s so the meters have real history"
sleep "$SETTLE"

# The window's own pixmap, not a screen region: nothing raised over it can land in the shot,
# and the client window is exactly the drawable area, with no frame or title bar.
xwd -id "$WIN" -silent > "$WORK/shot.xwd"
magick "$WORK/shot.xwd" "$OUT"
eval "$(xdotool getwindowgeometry --shell "$WIN")"
W=$WIDTH H=$HEIGHT
log "wrote $OUT (${W}x${H})"
