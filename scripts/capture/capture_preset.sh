#!/usr/bin/env bash
# Record one of YOUR presets, a group of layers at a time, with its audio.
#
# Loads a saved preset, waits until every video layer in it has decoded, then for each
# --step shows ONLY that step's layers (every other layer is switched off), lets them
# settle, and records a clip of the canvas with the music Fosfora is hearing mixed in.
#
#   scripts/capture/capture_preset.sh --preset Flovid --step 0,1 --step 2,3 --step 4,5
#
# Layer numbers are 0-based and count from the TOP of the layer list, so the list's
# layers 1+2 are `--step 0,1`. Steps are exclusive on purpose: an effect that reads the
# layers beneath it (Fluvid) sees every enabled layer under it, so leaving an earlier
# pair on would bury the next one.
#
# Same guarantees as capture.sh, from the same library: an isolated config (your preset
# and its bindings sidecar are COPIED in; nothing in ~/.config is touched), a private
# null sink with only Fosfora's own capture stream moved onto it (the default sink is
# never modified), and a loopback so you hear what is being recorded (--no-listen to
# silence it). x11grab films a screen region: leave the desktop alone while it runs.
#
# Options:
#   --preset NAME     a preset saved in ~/.config/fosfora/presets (required)
#   --step A,B,…      layers to show for one clip; repeat for each clip (required)
#   --secs N          length of each clip (default 20)
#   --settle N        seconds between switching a step on and recording it (default 5)
#   --audio FILE      music to play, looped (default: the synthesized 124 BPM demo loop)
#   -o, --out DIR     where the clips go (default ./capture-out-preset)
#   --no-listen       don't play the music on your speakers while recording

set -Eeuo pipefail

REPO=$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
# shellcheck disable=SC2034  # read by _capture_lib.sh's log/die
CAP_TAG=preset
# shellcheck source=/dev/null
source "$REPO/scripts/capture/_capture_lib.sh"

PRESET= SECS=20 SETTLE=5 AUDIO= LISTEN=1 FPS=60
OUT=${OUT:-$PWD/capture-out-preset}
STEPS=()
while [[ $# -gt 0 ]]; do
  case $1 in
    --preset)     PRESET=$2; shift 2 ;;
    --step)       STEPS+=("$2"); shift 2 ;;
    --secs)       SECS=$2; shift 2 ;;
    --settle)     SETTLE=$2; shift 2 ;;
    --audio)      AUDIO=$2; shift 2 ;;
    -o|--out)     OUT=$2; shift 2 ;;
    --no-listen)  LISTEN=0; shift ;;
    -h|--help)    sed -n '2,32p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[[ -n $PRESET ]] || die "--preset NAME is required"
(( ${#STEPS[@]} > 0 )) || die "at least one --step is required, e.g. --step 0,1"

USER_PRESETS=${XDG_CONFIG_HOME:-$HOME/.config}/fosfora/presets
PFILE=$USER_PRESETS/$PRESET.json
[[ -f $PFILE ]] || die "no preset '$PRESET' in $USER_PRESETS"
[[ -z $AUDIO || -f $AUDIO ]] || die "audio file not found: $AUDIO"

cap_preflight_tools ffmpeg xdotool xwininfo oscsend pactl python3
BIN=$REPO/target/release/fosfora
cap_require_fresh_binary "$BIN" "$REPO"
# OSC is how this drives the app, on port 9000; a running Fosfora would take the
# messages instead, and be switched around by them.
if pgrep -x fosfora >/dev/null; then
  die "Fosfora is already running — close it first (it would receive this script's OSC)"
fi

# Layer count, and the video files the preset needs: every one has to exist, and the
# run waits for each to finish decoding before it films anything.
# Counts on the first line, missing files (paths may contain spaces) on the rest.
mapfile -t INFO < <(python3 - "$PFILE" <<'EOF'
import json, os, sys
layers = json.load(open(sys.argv[1]))["layers"]
media = [l["media_path"] for l in layers if l.get("media_path")]
print(len(layers), len(media))
for m in media:
    if not os.path.isfile(m):
        print(m)
EOF
)
read -r NLAYERS NMEDIA <<<"${INFO[0]}"
(( ${#INFO[@]} == 1 )) || die "the preset's media is missing: ${INFO[*]:1}"
for s in "${STEPS[@]}"; do
  for i in ${s//,/ }; do
    [[ $i =~ ^[0-9]+$ ]] && (( i < NLAYERS )) \
      || die "--step $s: layer $i is not one of the preset's $NLAYERS layers (0-$((NLAYERS-1)))"
  done
done

WORK=$(mktemp -d -t fosfora-preset-XXXXXX)
CFG=$WORK/cfg
SINK=fosfora_preset
SCENE_NAME="Preset Capture"
APP_PID= PLAY_PID= SINK_MOD= LOOPBACK_MOD=

cleanup() {
  local rc=$?
  log "cleaning up…"
  [[ -n $PLAY_PID ]] && kill "$PLAY_PID" 2>/dev/null || true
  [[ -n $APP_PID  ]] && kill "$APP_PID"  2>/dev/null || true
  sleep 0.5
  [[ -n $APP_PID  ]] && kill -9 "$APP_PID" 2>/dev/null || true
  # Unload only the modules we loaded, monitor first — never touch the default sink.
  [[ -n $LOOPBACK_MOD ]] && pactl unload-module "$LOOPBACK_MOD" 2>/dev/null || true
  [[ -n $SINK_MOD     ]] && pactl unload-module "$SINK_MOD"     2>/dev/null || true
  mkdir -p "$OUT" && cp "$WORK/app.log" "$OUT/app.log" 2>/dev/null || true
  rm -rf "$WORK"
  exit $rc
}
trap cleanup EXIT INT TERM

DEFAULT_SINK=$(pactl get-default-sink)
log "default sink is '$DEFAULT_SINK' — it will NOT be modified"
mkdir -p "$OUT" "$CFG/fosfora/presets" "$CFG/fosfora/scenes"

# The preset loads BY NAME through a one-cue scene: next_preset would land on whatever
# index the scan order puts first, which is not ours to choose. Presets and scenes are
# scanned once at startup, so both go in before launch.
cp "$PFILE" "$CFG/fosfora/presets/"
[[ -f $USER_PRESETS/$PRESET.bindings.json ]] && cp "$USER_PRESETS/$PRESET.bindings.json" "$CFG/fosfora/presets/"
python3 - "$CFG/fosfora/scenes/$SCENE_NAME.json" "$SCENE_NAME" "$PRESET" <<'EOF'
import json, sys
json.dump({"version": 1, "name": sys.argv[2], "loop_mode": False, "advance_mode": "Manual",
           "cues": [{"preset_name": sys.argv[3], "transition": "Cut", "label": sys.argv[3]}]},
          open(sys.argv[1], "w"), indent=2)
EOF

if [[ -z $AUDIO ]]; then
  AUDIO=$WORK/loop.wav
  log "synthesizing the demo loop…"
  "$REPO/scripts/capture/make_loop.py" -o "$AUDIO" >&2
fi

# --------------------------------------------------------------------- launch
log "creating private null sink '$SINK'"
SINK_MOD=$(cap_make_sink "$SINK")

log "launching app with isolated config ($CFG)"
XDG_CONFIG_HOME=$CFG RUST_LOG=fosfora=info "$BIN" >"$WORK/app.log" 2>&1 &
APP_PID=$!
WIN=$(cap_wait_for_window "$APP_PID" "$WORK/app.log")
log "window $WIN up; letting it warm up"
sleep 8

cap_route_app_audio "$SINK"
if (( LISTEN )); then
  LOOPBACK_MOD=$(cap_start_monitor "$SINK" "$DEFAULT_SINK")
  [[ -n $LOOPBACK_MOD ]] && log "monitoring on '$DEFAULT_SINK' (--no-listen to silence)" \
                         || log "WARNING: could not open the monitor path; continuing silently"
fi
log "starting audio playback: $(basename "$AUDIO")"
ffmpeg -hide_banner -loglevel error -re -stream_loop -1 -i "$AUDIO" \
       -f pulse -device "$SINK" fosfora-preset </dev/null &
PLAY_PID=$!

xdotool windowactivate --sync "$WIN"; sleep 0.6
osc() { oscsend localhost 9000 "$@"; }
# SET the UI hidden rather than toggling it, and again before every clip: see
# capture_advanced.sh for the run that was filmed through the panels.
hide_ui() { osc /fosfora/overlay/visible f 0.0; }
hide_ui; sleep 1.5

# Canvas detection needs something animating edge to edge, which the preset may not
# have (Fluvid over a still frame is mostly black). Aurora, two effects in from boot,
# fills the frame; detect on it, then load the preset.
osc /fosfora/trigger/next_effect f 1.0; sleep 0.8
osc /fosfora/trigger/next_effect f 1.0; sleep 3.0
read -r X Y W H < <(cap_detect_canvas "$REPO" "$WIN")
log "capture geometry ${W}x${H}+${X}+${Y}"
cap_region_check "$OUT/_region_check.png" "$X" "$Y" "$W" "$H"

# --------------------------------------------------------------------- preset
MARK=$(stat -c%s "$WORK/app.log")
osc /fosfora/scene/load s "$SCENE_NAME"
log "loading preset '$PRESET' ($NMEDIA video layer(s) to decode)…"
ok=0
for _ in $(seq 180); do
  SLOT_LOG=$(tail -c "+$((MARK+1))" "$WORK/app.log")
  if grep -qF "Loaded preset '$PRESET'" <<<"$SLOT_LOG" \
     && (( $(grep -c "loaded media .*(pre-decoded)" <<<"$SLOT_LOG" || true) >= NMEDIA )); then
    ok=1; break
  fi
  if grep -qE "Failed to load media|Load error:|Failed to load effect" <<<"$SLOT_LOG"; then
    die "the preset did not load cleanly: $(grep -oE "(Failed to load media|Load error:|Failed to load effect).*" <<<"$SLOT_LOG" | head -1)"
  fi
  kill -0 "$APP_PID" 2>/dev/null || die "the app exited while loading the preset"
  sleep 0.5
done
(( ok )) || die "the preset did not finish loading within 90 s (see $OUT/app.log)"
log "preset loaded"

# --------------------------------------------------------------------- clips
k=0
for s in "${STEPS[@]}"; do
  k=$((k+1))
  want=" ${s//,/ } "
  for (( i = 0; i < NLAYERS; i++ )); do
    if [[ $want == *" $i "* ]]; then osc /fosfora/layer/$i/enabled f 1.0
    else osc /fosfora/layer/$i/enabled f 0.0; fi
  done
  hide_ui
  # x11grab films a region, not a window: re-raise so a focus steal costs one clip at most.
  xdotool windowactivate --sync "$WIN" 2>/dev/null || true
  xdotool windowraise "$WIN" 2>/dev/null || true
  name=$(printf '%s_%d_layers_%s' "$PRESET" "$k" "${s//,/-}" | tr ' ' '_')
  log "step $k/${#STEPS[@]}: layers $s on; settling ${SETTLE}s"
  sleep "$SETTLE"
  log "recording $name.mp4 (${SECS}s)"
  # Video from the canvas, audio from the private sink's monitor: exactly what the app hears.
  ffmpeg -hide_banner -loglevel error -y \
         -thread_queue_size 1024 -f x11grab -draw_mouse 0 -framerate $FPS \
         -video_size "${W}x${H}" -i "$DISPLAY+$X,$Y" \
         -thread_queue_size 1024 -f pulse -i "$SINK.monitor" \
         -t "$SECS" -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p \
         -c:a aac -b:a 192k "$OUT/$name.mp4" </dev/null
  log "  -> $(du -h "$OUT/$name.mp4" | cut -f1)"
done

log "done — clips in $OUT"
