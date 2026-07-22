#!/usr/bin/env bash
# Capture one clip per built-in effect by driving the real Fosfora app.
#
# Why the live app and not an offscreen probe: the `#[ignore]`d GPU probes in the test suite
# are each hardcoded to a single effect and bypass the post-process chain entirely, so their
# output does not look like what a user sees. Filming the running app gives true default
# settings through the production pipeline — bloom, tonemap, vignette, grain and all.
#
# How it stays honest:
#   * Config is ISOLATED via XDG_CONFIG_HOME, so the app starts from stock defaults and the
#     run cannot read or write the operator's real presets, bindings or settings.
#   * Audio is routed by MOVING the app's own capture stream onto a private null sink. The
#     system default sink is never touched — someone is usually working at this machine.
#   * The loop is played once through `-stream_loop -1`, so playback never drifts, and each
#     effect is filmed over exactly one loop pass. Every clip therefore covers the identical
#     musical passage (groove -> buildup -> gap -> drop), which is what makes the gallery a
#     fair comparison instead of a lottery over whichever bar happened to be playing.
#     That phase lock is load-bearing and easy to lose: see the scheduling note further down —
#     a fixed per-effect sleep CANNOT hold it, and when it slipped, clips silently landed on
#     different parts of the music and late ones missed the drop altogether.
#
# Effect order is deterministic: `EffectLoader::scan_effects_directory` sorts by .pfx filename
# and cycling skips `hidden` effects. The app boots on Phosphor (hidden), so the first
# next_effect lands on visible[1], not visible[0] — hence the wrap at the end.
#
# Usage:  scripts/capture/capture.sh [-o OUTDIR] [-s SECONDS] [--effects N] [--dry-run]

set -Eeuo pipefail

OUT=${OUT:-$PWD/capture-out}
LOOP_BARS=20
BPM=124
LOOP_SECS=$(python3 -c "print(f'{$LOOP_BARS*4*60/$BPM:.6f}')")
SETTLE_FRAC=0.5   # first half of the loop settles the effect, second half is filmed
FPS=60
DRY=0
LIMIT=0
LISTEN=1

while [[ $# -gt 0 ]]; do
  case $1 in
    -o|--out)     OUT=$2; shift 2 ;;
    --effects)    LIMIT=$2; shift 2 ;;
    --dry-run)    DRY=1; shift ;;
    --no-listen)  LISTEN=0; shift ;;
    -h|--help)    sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

REPO=$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
BIN=$REPO/target/release/phosphor-app
WORK=$(mktemp -d -t fosfora-capture-XXXXXX)
CFG=$WORK/cfg
SINK=fosfora_cap
APP_PID=  PLAY_PID=  SINK_MOD=  LOOPBACK_MOD=

log() { printf '\033[36m[capture]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31m[capture] %s\033[0m\n' "$*" >&2; exit 1; }

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
for t in ffmpeg xdotool xwininfo oscsend pactl python3; do
  command -v "$t" >/dev/null || die "missing required tool: $t"
done
[[ -x $BIN ]] || die "release binary not found at $BIN — run: cargo build --release"
[[ -n ${DISPLAY:-} ]] || die "no DISPLAY; this script drives a real X11 window"

# A stale binary does not fail loudly here, because shaders are loaded from assets/ at RUNTIME:
# a target/ build from before the last few commits happily runs the NEW .wgsl against its OLD
# Rust and renders something plausible. That cost a full 38-clip run once — the binary predated
# the sorted 3DGS renderer, and the only visible symptom was Splat rendering flat grey.
if [[ -n $(find "$REPO/crates" -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null) ]]; then
  die "$BIN is older than crates/*.rs — the clips would not be of the current build.
     Run: cargo build --release --features \"video,ndi,webcam,depth\""
fi

DEFAULT_SINK=$(pactl get-default-sink)
log "default sink is '$DEFAULT_SINK' — it will NOT be modified"

mkdir -p "$OUT" "$CFG/phosphor/splats"

# Effect cycle order, straight from the .pfx files the app will scan.
mapfile -t NAMES < <(python3 - "$REPO/assets/effects" <<'PY'
import json, pathlib, sys
d = pathlib.Path(sys.argv[1])
for p in sorted(d.glob("*.pfx"), key=lambda p: p.name):      # matches entries.sort_by_key(file_name)
    e = json.loads(p.read_text())
    if not e.get("hidden"):                                   # cycling skips hidden effects
        print(f"{p.stem}\t{e['name']}")
PY
)
N=${#NAMES[@]}
(( N > 0 )) || die "no effects found"
# Cycling always walks the full ring, so --effects just stops early rather than shrinking it —
# shrinking N would corrupt the wrap-around index and mislabel every clip.
TAKE=$N
(( LIMIT > 0 && LIMIT < N )) && TAKE=$LIMIT
log "$TAKE of $N effects; loop ${LOOP_SECS}s; ~$(python3 -c "print(f'{$TAKE*$LOOP_SECS/60:.1f}')") min of capture"

# --------------------------------------------------------------------- audio loop
LOOPWAV=$WORK/loop.wav
log "synthesizing demo loop…"
"$REPO/scripts/capture/make_loop.py" -o "$LOOPWAV" --bpm "$BPM" --bars "$LOOP_BARS" >&2

# Splat ships "source": "demo:default", which resolves to phosphor_demo.ply under the config
# dir. Isolating the config means it is not there — fetch it so Splat renders a scene.
DEMO_PLY=$CFG/phosphor/splats/phosphor_demo.ply
DEMO_URL=https://github.com/kevinraymond/fosfora/releases/download/demo-assets/trooper.ply
if [[ ! -f $DEMO_PLY ]]; then
  CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/fosfora-capture/phosphor_demo.ply
  if [[ -f $CACHE ]]; then
    log "using cached splat demo scene"
    cp "$CACHE" "$DEMO_PLY"
  else
    log "downloading splat demo scene (~42 MB)…"
    mkdir -p "$(dirname "$CACHE")"
    curl -fsSL "$DEMO_URL" -o "$CACHE" && cp "$CACHE" "$DEMO_PLY" \
      || log "WARNING: splat demo download failed; Splat will render empty"
  fi
fi

if (( DRY )); then
  log "dry run — effect order:"
  for ((i=0;i<N;i++)); do printf '  %2d  %s\n' $((i+1)) "${NAMES[i]}" >&2; done
  exit 0
fi

# --------------------------------------------------------------------- launch
log "creating private null sink '$SINK'"
SINK_MOD=$(pactl load-module module-null-sink sink_name=$SINK \
             sink_properties=device.description=FosforaCapture)

log "launching app with isolated config ($CFG)"
XDG_CONFIG_HOME=$CFG RUST_LOG=phosphor_app=info "$BIN" >"$WORK/app.log" 2>&1 &
APP_PID=$!

# Under a reparenting WM, `xdotool search` matches BOTH the WM frame and the client window.
# Taking the first match grabbed the frame (2042x1239) and filmed a border of desktop around
# the canvas. The frame always encloses the client, so the smallest sane match is the client.
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
[[ -n $WIN ]] || die "app window never appeared; see $WORK/app.log"
log "window $WIN up; letting it warm up"
sleep 8

# Route the app's capture stream onto our null sink. This moves only Fosfora's own stream;
# every other client, and the default sink itself, is left exactly as it was.
SO=$(pactl -f json list source-outputs 2>/dev/null \
      | python3 -c "
import json,sys
for so in json.load(sys.stdin):
    props = so.get('properties', {})
    if 'fosfora' in json.dumps(props).lower() or 'phosphor' in json.dumps(props).lower():
        print(so['index']); break
")
[[ -n $SO ]] || die "could not find Fosfora's capture stream in pactl source-outputs"
pactl move-source-output "$SO" "$SINK.monitor"
log "moved app capture stream #$SO -> $SINK.monitor"

# Monitoring: bridge the capture sink to whatever the default sink already is, so the loop is
# audible while the run proceeds. This ADDS a path; it does not reroute anything. The default
# sink is not changed, no other application is moved, and the bridge is torn down on exit.
if (( LISTEN )); then
  LOOPBACK_MOD=$(pactl load-module module-loopback \
                   source="$SINK.monitor" sink="$DEFAULT_SINK" latency_msec=60 2>/dev/null) \
    && log "monitoring on '$DEFAULT_SINK' (--no-listen to silence)" \
    || log "WARNING: could not open the monitor path; continuing silently"
fi

log "starting audio playback (seamless, no drift)"
ffmpeg -hide_banner -loglevel error -re -stream_loop -1 -i "$LOOPWAV" \
       -f pulse -device "$SINK" fosfora-capture &
PLAY_PID=$!
PLAY_T0=$(date +%s.%N)   # everything downstream is scheduled against this

# Deliberately NOT fullscreen: this display is 4K, and x11grab feeding x264 at 3840x2160p60
# drops frames, which would show up as stutter in the very clips meant to advertise smoothness.
# Pin the window to exactly 1920x1080 instead so the grab is a known, correctly-sized 16:9 canvas.
# Do NOT resize the window. It already opens at exactly 1920x1080, and asking xdotool to
# resize the client makes this WM apply the size to the *frame* instead — which silently
# shrinks the drawable area and leaves a strip of desktop inside the grab.
xdotool windowactivate --sync "$WIN"; sleep 0.6

# Hide the UI overlay via OSC rather than the `d` key: xdotool key events do not reliably reach
# this window (the first smoke run captured a full set of panels), whereas the OSC trigger goes
# straight into the same handler the keyboard would have.
oscsend localhost 9000 /phosphor/trigger/toggle_overlay f 1.0; sleep 1.5

step() { oscsend localhost 9000 /phosphor/trigger/next_effect f 1.0; }

# Step to Aurora before measuring. Motion detection needs something that animates edge to edge:
# measuring against Array (a dark centre column on black) found only the lit middle and
# under-reported the canvas by a third. Aurora's curtain bands fill the frame.
# Boot is Phosphor (hidden) -> visible[1] Array -> visible[2] Aurora.
step; sleep 0.8; step; sleep 3.0

# Find the canvas by motion rather than by asking the WM — see find_canvas.py for why. Search
# the union of every window the app owns, which is guaranteed to enclose the client area.
SX=999999 SY=999999 SR=0 SB=0
for w in $(xdotool search --name '^Fosfora$'); do
  eval "$(xdotool getwindowgeometry --shell "$w" 2>/dev/null)" || continue
  (( X  < SX )) && SX=$X
  (( Y  < SY )) && SY=$Y
  (( X + WIDTH  > SR )) && SR=$((X + WIDTH))
  (( Y + HEIGHT > SB )) && SB=$((Y + HEIGHT))
done
log "searching for canvas within ${SX},${SY} $((SR-SX))x$((SB-SY))"
# Pass the known canvas size so find_canvas.py searches for WHERE that fixed-size window sits,
# rather than taking a bounding box of all motion. A bounding box is not stable: on one run a
# terminal repainting just outside the window dragged the top edge up by 45 px and put a strip
# of title bar into all 38 clips.
eval "$(xdotool getwindowgeometry --shell "$WIN")"
read -r X Y W H < <("$REPO/scripts/capture/find_canvas.py" \
                      --region "$SX" "$SY" "$((SR-SX))" "$((SB-SY))" \
                      --size "$WIDTH" "$HEIGHT" --display "$DISPLAY") \
  || die "could not locate the canvas (nothing on screen was animating)"

# Take the ORIGIN from motion (the WM's reported client position is off by the reparent offset)
# but cap the SIZE at the client window's own dimensions. Unrelated desktop motion just outside
# the window — a terminal repainting — can otherwise widen the box by a few tens of pixels and
# bleed a strip of desktop into every clip.
CX=$X CY=$Y                                  # keep the detected origin: the eval below reuses X/Y
eval "$(xdotool getwindowgeometry --shell "$WIN")"
X=$CX Y=$CY
(( W > WIDTH  )) && W=$WIDTH
(( H > HEIGHT )) && H=$HEIGHT
W=$((W - W % 2)); H=$((H - H % 2))
log "capture geometry ${W}x${H}+${X}+${Y}  (origin by motion, size from client window)"
[[ $W -ge 640 && $H -ge 360 ]] || die "detected canvas ${W}x${H} is implausibly small"

# Save a still of exactly what will be filmed, so a bad region is caught here rather than
# discovered 20 minutes later in 38 clips.
ffmpeg -hide_banner -loglevel error -y -f x11grab -video_size "${W}x${H}" \
       -i "$DISPLAY+$X,$Y" -frames:v 1 "$OUT/_region_check.png" </dev/null
log "wrote region check: $OUT/_region_check.png"

REC_SECS=$(python3 -c "print(f'{$LOOP_SECS*(1-$SETTLE_FRAC):.3f}')")

# Schedule every step against an ABSOLUTE clock rather than sleeping a fixed amount per effect.
#
# The first version slept `LOOP_SECS*SETTLE_FRAC - 1.2` and then recorded `LOOP_SECS/2`, the 1.2
# being a fudge for effect-switch and ffmpeg startup. That makes the cadence 37.5s against a
# 38.7s loop — a 1.2s slip per effect, 45.6s across 38 effects, which is more than a whole loop.
# The phase lock the whole design rests on quietly did not exist: each clip landed on a
# different part of the music, and clips late in the run missed the drop entirely (Vessel's
# burst is ~1s long and simply fell outside its window).
#
# Anchoring each boundary to T0 + k*LOOP_SECS absorbs whatever the per-iteration overhead is.
# T0 is PLAYBACK start, not "now" — that is what ties the schedule to the music. The loop puts
# its drop at 0.75 of a pass, so a record window of [0.5, 1.0] brackets it with margin, and
# every effect gets the buildup, the pre-drop gap and the drop.
now() { date +%s.%N; }
wait_until() {   # wait_until <seconds since PLAY_T0>
  local d
  d=$(python3 -c "print(max(0.0, $PLAY_T0 + $1 - $(now)))")
  python3 -c "
import sys
if $d <= 0.0: sys.stderr.write('  [warn] behind schedule, phase lock lost\n')"
  sleep "$d"
}

# Skip whole passes until the next slot is safely in the future — setup (canvas detection,
# the demo download) has already consumed some of the timeline.
PASS0=$(python3 -c "
import math
elapsed = $(now) - $PLAY_T0
print(max(1, math.ceil((elapsed + 3.0) / $LOOP_SECS)))")
log "phase-locked to playback; first record window opens in pass $PASS0"

# Canvas detection left us on Aurora = visible[2], so capture starts there and wraps all the
# way round, covering every effect exactly once.
for ((k=0;k<TAKE;k++)); do
  idx=$(( (k+2) % N ))
  IFS=$'\t' read -r slug name <<<"${NAMES[idx]}"
  printf '\033[36m[capture]\033[0m %2d/%d  %-22s' $((k+1)) "$TAKE" "$name" >&2

  # Record window opens at a fixed offset into each loop pass, so every effect is filmed over
  # the same bars — which is the claim docs/GALLERY.md makes about these clips.
  wait_until "$(python3 -c "print(($PASS0+$k)*$LOOP_SECS + $LOOP_SECS*$SETTLE_FRAC)")"
  ffmpeg -hide_banner -loglevel error -y \
         -f x11grab -framerate $FPS -video_size "${W}x${H}" -i "$DISPLAY+$X,$Y" \
         -t "$REC_SECS" -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p \
         "$OUT/$slug.mp4" </dev/null
  printf ' -> %s\n' "$(du -h "$OUT/$slug.mp4" | cut -f1)" >&2

  # Switch during the next pass's settle half, then wait out the rest of the slot.
  step
done

log "done — $TAKE clips in $OUT"
cp "$WORK/app.log" "$OUT/app.log" 2>/dev/null || true
