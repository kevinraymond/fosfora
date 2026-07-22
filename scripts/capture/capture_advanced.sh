#!/usr/bin/env bash
# Capture one clip per ADVANCED demo — the things a default preset cannot show:
# obstacle collision, a blended layer stack, volumetric mode, custom audio bindings,
# a media layer, and a scene cue dissolve.
#
# Same guarantees as capture.sh, from the same library: isolated config so the run
# starts from stock defaults and cannot touch the operator's presets; a private null
# sink with only Fosfora's own stream moved onto it, so the system default sink is never
# modified; and one continuous `-stream_loop -1` playback with every boundary scheduled
# against an absolute clock, so all clips cover the identical musical passage.
#
# Demos are driven by a SCENE CUE LIST, not by next_preset. From a cold boot
# `current_preset` is None, so the first next_preset lands on index 1, and any
# advance-by-count choreography is hostage to how many built-in presets ship.
# `/phosphor/scene/goto_cue` resolves the preset BY NAME through our own scene file, so
# the mapping is ours. It also gives the dissolve demo for free.
#
# Presets, the bindings sidecar and the scene are written into the isolated config
# BEFORE launch — PresetStore::scan and SceneStore::scan each run once, at startup.
#
# Usage:  scripts/capture/capture_advanced.sh [-o OUTDIR] [--dry-run] [--stills-only]
#                                             [--only SLUG] [--no-listen] [--no-strict]

set -Eeuo pipefail

REPO=$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
# shellcheck disable=SC2034  # read by _capture_lib.sh's log/die
CAP_TAG=advanced
# shellcheck source=/dev/null
source "$REPO/scripts/capture/_capture_lib.sh"

OUT=${OUT:-$PWD/capture-out-advanced}
DEMOS=$REPO/scripts/capture/demos
SCENE_NAME="Fosfora Advanced"
LOOP_BARS=20
BPM=124
LOOP_SECS=$(python3 -c "print(f'{$LOOP_BARS*4*60/$BPM:.6f}')")
SETTLE_FRAC=0.5     # first half of the loop settles the demo, second half is filmed
FPS=60
# The tile build_media.sh will cut, relative to the start of each clip. Everything the
# demo needs to be doing has to be happening inside this window.
TILE_AT=9.75
TILE_DUR=$(python3 -c "print(f'{2*4*60/$BPM:.4f}')")
DRY=0 STILLS=0 LISTEN=1 STRICT=1 ONLY=

while [[ $# -gt 0 ]]; do
  case $1 in
    -o|--out)       OUT=$2; shift 2 ;;
    --dry-run)      DRY=1; shift ;;
    --stills-only)  STILLS=1; shift ;;
    --only)         ONLY=$2; shift 2 ;;
    --no-listen)    LISTEN=0; shift ;;
    --no-strict)    STRICT=0; shift ;;
    -h|--help)      sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

BIN=$REPO/target/release/phosphor-app
WORK=$(mktemp -d -t fosfora-adv-XXXXXX)
CFG=$WORK/cfg
SINK=fosfora_adv
APP_PID=  PLAY_PID=  SINK_MOD=  LOOPBACK_MOD=

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
  rm -rf "$WORK"
  exit $rc
}
trap cleanup EXIT INT TERM

# --------------------------------------------------------------------- preflight
# --dry-run first, and before anything that needs a binary, a display or PulseAudio:
# it is meant to be runnable anywhere, including CI.
if (( DRY )); then
  exec "$REPO/scripts/capture/check_demos.py"
fi

cap_preflight_tools ffmpeg xdotool xwininfo oscsend pactl python3
cap_require_fresh_binary "$BIN" "$REPO"

"$REPO/scripts/capture/check_demos.py" --quiet \
  || die "demo presets did not validate — fix those first, every one of them fails silently at runtime"

# slug \t cue \t preset \t required-log-regex
mapfile -t ROWS < <(grep -v '^\s*#' "$DEMOS/_manifest.tsv" | grep -v '^\s*$')
N=${#ROWS[@]}
(( N > 0 )) || die "no rows in _manifest.tsv"

DEFAULT_SINK=$(pactl get-default-sink)
log "default sink is '$DEFAULT_SINK' — it will NOT be modified"
mkdir -p "$OUT" "$CFG/phosphor/presets" "$CFG/phosphor/scenes"

# --------------------------------------------------------------------- demo state
# @REPO@ / @WORK@ are substituted here rather than baked in, so the committed demo files
# stay portable — a user can copy them into ~/.config/phosphor/presets/ and get the same
# looks, which is half the point of shipping them as files.
log "installing demo presets into the isolated config"
for f in "$DEMOS"/*.json; do
  b=$(basename "$f")
  [[ $b == _* ]] && continue
  sed "s|@REPO@|$REPO|g; s|@WORK@|$WORK|g" "$f" > "$CFG/phosphor/presets/$b"
done
sed "s|@REPO@|$REPO|g; s|@WORK@|$WORK|g" "$DEMOS/_scene.json" \
  > "$CFG/phosphor/scenes/$SCENE_NAME.json"

# The media demo needs a 16:9 still. Generating it beats pointing the layer at a shipped
# 2048x2048 PNG, because MediaLayer letterboxes — a square image would pillarbox into a
# 1080x1080 island with black down both sides, which is not what a media layer looks like
# in use.
ffmpeg -hide_banner -loglevel error -y -i "$REPO/assets/images/raster_magic_tree.png" \
  -vf "scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080" \
  "$WORK/media_bg.png" </dev/null

# --------------------------------------------------------------------- audio loop
LOOPWAV=$WORK/loop.wav
log "synthesizing demo loop…"
"$REPO/scripts/capture/make_loop.py" -o "$LOOPWAV" --bpm "$BPM" --bars "$LOOP_BARS" >&2

# --------------------------------------------------------------------- launch
log "creating private null sink '$SINK'"
SINK_MOD=$(cap_make_sink "$SINK")

log "launching app with isolated config ($CFG)"
XDG_CONFIG_HOME=$CFG RUST_LOG=phosphor_app=info "$BIN" >"$WORK/app.log" 2>&1 &
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

log "starting audio playback (seamless, no drift)"
ffmpeg -hide_banner -loglevel error -re -stream_loop -1 -i "$LOOPWAV" \
       -f pulse -device "$SINK" fosfora-advanced &
PLAY_PID=$!
PLAY_T0=$(cap_now)   # everything downstream is scheduled against this

# Deliberately NOT fullscreen, and deliberately not resized: see capture.sh. The window
# already opens at exactly 1920x1080, and asking xdotool to resize the client makes this
# WM apply the size to the *frame*.
xdotool windowactivate --sync "$WIN"; sleep 0.6

osc() { oscsend localhost 9000 "$@"; }

# Hide the UI via OSC rather than the `d` key: xdotool key events do not reliably reach
# this window, whereas the OSC trigger goes straight into the same handler.
osc /phosphor/trigger/toggle_overlay f 1.0; sleep 1.5

# Canvas detection needs something animating edge to edge, and boot lands on Phosphor
# (hidden) -> Array (a dark centre column on black, which under-reports the canvas by a
# third) -> Aurora, whose curtain bands fill the frame.
osc /phosphor/trigger/next_effect f 1.0; sleep 0.8
osc /phosphor/trigger/next_effect f 1.0; sleep 3.0

read -r X Y W H < <(cap_detect_canvas "$REPO" "$WIN")
log "capture geometry ${W}x${H}+${X}+${Y}  (origin by motion, size from client window)"
cap_region_check "$OUT/_region_check.png" "$X" "$Y" "$W" "$H"

REC_SECS=$(python3 -c "print(f'{$LOOP_SECS*(1-$SETTLE_FRAC):.3f}')")
PASS0=$(cap_first_pass "$PLAY_T0" "$LOOP_SECS")
log "phase-locked to playback; first record window opens in pass $PASS0"

# Forbidden anywhere in a slot's log. Note the startup-time "Failed to parse preset
# …bindings.json" is NOT in this set: sidecars live in presets/ and PresetStore::scan
# tries to deserialize them as presets, which is harmless and expected.
FORBIDDEN='not found for layer|Failed to load effect|Load error:|Failed to load obstacle image|not found for cue|Splat scene load failed'

k=0
for row in "${ROWS[@]}"; do
  IFS=$'\t' read -r slug cue preset want <<<"$row"
  if [[ -n $ONLY && $ONLY != "$slug" ]]; then k=$((k+1)); continue; fi

  S=$(python3 -c "print(($PASS0+$k)*$LOOP_SECS)")
  printf '\033[36m[advanced]\033[0m %d/%d  %-16s' $((k+1)) "$N" "$slug" >&2

  # --- slot opens: switch demo -------------------------------------------------
  cap_wait_until "$PLAY_T0" "$S"
  # x11grab films a screen REGION, not a window, so anything raised over the app lands
  # in the clip: one rehearsal run caught a terminal across the bottom third of the media
  # demo. Re-raise every slot so a transient focus steal costs at most part of one clip.
  # It cannot defend against someone actively using the desktop — that needs the screen
  # left alone for the length of the run.
  xdotool windowactivate --sync "$WIN" 2>/dev/null || true
  xdotool windowraise "$WIN" 2>/dev/null || true
  MARK=$(stat -c%s "$WORK/app.log")
  if (( k == 0 )); then
    osc /phosphor/scene/load s "$SCENE_NAME"
  else
    # go_to_cue returns None when already on that index, so never re-issue one.
    osc /phosphor/scene/goto_cue i "$cue"
  fi

  # Belt and braces against the morph-safe skip carrying obstacle state across a cut:
  # check_demos.py already refuses adjacent cues that share an effect on a layer index,
  # but the obstacle lives on the particle system rather than in the preset diff.
  cap_wait_until "$PLAY_T0" "$(python3 -c "print($S + 0.5)")"
  if [[ $slug != adv_obstacle ]]; then
    for n in 0 1 2 3; do osc /phosphor/layer/$n/obstacle/enabled f 0.0; done
  fi

  # --- settle gate -------------------------------------------------------------
  cap_wait_until "$PLAY_T0" "$(python3 -c "print($S + 3.0)")"
  SLOT_LOG=$(tail -c "+$((MARK+1))" "$WORK/app.log")
  fail=
  grep -qF "Loaded preset '$preset'" <<<"$SLOT_LOG" || fail="preset '$preset' never loaded"
  if [[ -z $fail && -n $want ]]; then
    grep -qE "$want" <<<"$SLOT_LOG" || fail="expected log line missing: $want"
  fi
  if [[ -z $fail ]] && grep -qE "$FORBIDDEN" <<<"$SLOT_LOG"; then
    fail="error in log: $(grep -oE "$FORBIDDEN" <<<"$SLOT_LOG" | head -1)"
  fi
  if [[ -n $fail ]]; then
    printf ' \033[31mFAIL\033[0m %s\n' "$fail" >&2
    (( STRICT )) && die "aborting: $slug did not come up.
     A half-loaded effect also pins the shader-editor overlay open, which there is
     no OSC escape from, so every later clip would be filmed through it.
     Full log: $WORK/app.log (copied to $OUT/app.log)
     Re-run one demo with --only $slug, or --no-strict to push through."
  fi

  # --- confirmation still ------------------------------------------------------
  cap_wait_until "$PLAY_T0" "$(python3 -c "print($S + 4.0)")"
  ffmpeg -hide_banner -loglevel error -y -f x11grab -draw_mouse 0 -video_size "${W}x${H}" \
         -i "$DISPLAY+$X,$Y" -frames:v 1 "$OUT/_check_$slug.png" </dev/null

  if (( STILLS )); then
    # Rehearsal: grab a frame from INSIDE the tile window instead of recording. This is
    # the cheap answer to "is this visibly different from the default?", because it is
    # literally what the first frame of the tile will be.
    cap_wait_until "$PLAY_T0" "$(python3 -c "print($S + $LOOP_SECS*$SETTLE_FRAC + $TILE_AT + 0.2)")"
    mkdir -p "$OUT/_rehearsal"
    ffmpeg -hide_banner -loglevel error -y -f x11grab -draw_mouse 0 -video_size "${W}x${H}" \
           -i "$DISPLAY+$X,$Y" -frames:v 1 "$OUT/_rehearsal/$slug.png" </dev/null
    printf ' -> rehearsal still\n' >&2
    k=$((k+1)); continue
  fi

  # --- record ------------------------------------------------------------------
  cap_wait_until "$PLAY_T0" "$(python3 -c "print($S + $LOOP_SECS*$SETTLE_FRAC)")"
  ffmpeg -hide_banner -loglevel error -y \
         -f x11grab -draw_mouse 0 -framerate $FPS -video_size "${W}x${H}" -i "$DISPLAY+$X,$Y" \
         -t "$REC_SECS" -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p \
         "$OUT/$slug.mp4" </dev/null &
  REC_PID=$!

  # The dissolve has to land inside the tile, not merely inside the clip. The tile is
  # [TILE_AT, TILE_AT+TILE_DUR] from the start of the recording, so start a 3s crossfade
  # far enough ahead that its middle sits in the middle of the tile.
  if [[ $slug == adv_cue ]]; then
    cap_wait_until "$PLAY_T0" \
      "$(python3 -c "print($S + $LOOP_SECS*$SETTLE_FRAC + $TILE_AT + $TILE_DUR/2 - 1.5)")"
    osc /phosphor/scene/goto_cue i 6
  fi

  wait "$REC_PID"
  printf ' -> %s\n' "$(du -h "$OUT/$slug.mp4" | cut -f1)" >&2
  k=$((k+1))
done

cp "$WORK/app.log" "$OUT/app.log" 2>/dev/null || true
if (( STILLS )); then
  log "rehearsal stills in $OUT/_rehearsal — eyeball all six before the video run"
else
  log "done — clips in $OUT; next:"
  log "  scripts/capture/build_media.sh -i $OUT --only tiles --at $TILE_AT"
fi
