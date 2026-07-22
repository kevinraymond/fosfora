#!/usr/bin/env bash
# Mechanics shared by the capture scripts: preflight, config/audio isolation, window
# and canvas location, and the absolute-clock scheduler.
#
# Extracted from capture.sh, which is NOT (yet) converted to use it. That script
# generates the release gallery, roughly 40% of it is comments recording specific past
# failures, and its only proof is a 25-minute 38-clip re-run. This library is proved by
# the 4-minute advanced run instead; converging capture.sh onto it later is a mechanical
# change that can be gated on its own.
#
# Policy stays in the caller: what to film, in what order, on what cadence. Only
# machinery lives here.
#
# Callers must set CAP_TAG before sourcing (used in log/die output).

CAP_TAG=${CAP_TAG:-capture}

log() { printf '\033[36m[%s]\033[0m %s\n' "$CAP_TAG" "$*" >&2; }
die() { printf '\033[31m[%s] %s\033[0m\n' "$CAP_TAG" "$*" >&2; exit 1; }

# --------------------------------------------------------------------- preflight

cap_preflight_tools() {
  local t
  for t in "$@"; do
    command -v "$t" >/dev/null || die "missing required tool: $t"
  done
  [[ -n ${DISPLAY:-} ]] || die "no DISPLAY; this script drives a real X11 window"
}

# cap_require_fresh_binary <binary> <repo>
#
# A stale binary does not fail loudly on its own, because shaders are loaded from
# assets/ at RUNTIME: a target/ build from before the last few commits happily runs the
# NEW .wgsl against its OLD Rust and renders something plausible. That cost a full
# 38-clip run once — the binary predated the sorted 3DGS renderer, and the only visible
# symptom was Splat rendering flat grey. It matters more for the advanced demos than for
# the effect gallery, since those exercise Rust-side preset, binding and scene code
# rather than only runtime-loaded WGSL.
cap_require_fresh_binary() {
  local bin=$1 repo=$2
  [[ -x $bin ]] || die "release binary not found at $bin — run: cargo build --release"
  if [[ -n $(find "$repo/crates" -name '*.rs' -newer "$bin" -print -quit 2>/dev/null) ]]; then
    die "$bin is older than crates/*.rs — the clips would not be of the current build.
     Run: cargo build --release --features \"video,ndi,webcam,depth\""
  fi
}

# --------------------------------------------------------------------- audio

# cap_make_sink <sink_name> -> module id on stdout
cap_make_sink() {
  pactl load-module module-null-sink sink_name="$1" \
        sink_properties=device.description=FosforaCapture
}

# cap_route_app_audio <sink_name>
#
# Moves ONLY Fosfora's own capture stream onto the private sink. Every other client, and
# the default sink itself, is left exactly as it was — someone is usually working at
# this machine.
cap_route_app_audio() {
  local sink=$1 so
  so=$(pactl -f json list source-outputs 2>/dev/null \
        | python3 -c "
import json,sys
for so in json.load(sys.stdin):
    props = json.dumps(so.get('properties', {})).lower()
    if 'fosfora' in props or 'phosphor' in props:
        print(so['index']); break
")
  [[ -n $so ]] || die "could not find Fosfora's capture stream in pactl source-outputs"
  pactl move-source-output "$so" "$sink.monitor"
  log "moved app capture stream #$so -> $sink.monitor"
}

# cap_start_monitor <sink_name> <default_sink> -> module id on stdout (empty on failure)
#
# Bridges the capture sink to whatever the default sink already is, so the loop is
# audible while the run proceeds. This ADDS a path; it does not reroute anything.
cap_start_monitor() {
  pactl load-module module-loopback source="$1.monitor" sink="$2" latency_msec=60 2>/dev/null || true
}

# --------------------------------------------------------------------- window

# Under a reparenting WM, `xdotool search` matches BOTH the WM frame and the client
# window. Taking the first match grabbed the frame (2042x1239) and filmed a border of
# desktop around the canvas. The frame always encloses the client, so the smallest sane
# match is the client.
cap_find_client_window() {
  local best= best_area=99999999 w area
  for w in $(xdotool search --name '^Fosfora$' 2>/dev/null); do
    eval "$(xdotool getwindowgeometry --shell "$w" 2>/dev/null)" || continue
    (( WIDTH < 200 || HEIGHT < 200 )) && continue
    area=$(( WIDTH * HEIGHT ))
    (( area < best_area )) && { best_area=$area; best=$w; }
  done
  [[ -n $best ]] && printf '%s' "$best"
}

# cap_wait_for_window <app_pid> <logfile> -> window id on stdout
cap_wait_for_window() {
  local pid=$1 logfile=$2 win=
  for _ in $(seq 60); do
    win=$(cap_find_client_window) && [[ -n $win ]] && break
    kill -0 "$pid" 2>/dev/null || die "app exited early; see $logfile"
    sleep 1
  done
  [[ -n $win ]] || die "app window never appeared; see $logfile"
  printf '%s' "$win"
}

# cap_detect_canvas <repo> <window> -> "X Y W H" on stdout
#
# Finds the canvas by MOTION rather than by asking the WM (the reported client position
# is off by the reparent offset). Searches the union of every window the app owns, which
# is guaranteed to enclose the client area, and passes the known canvas size so
# find_canvas.py locates WHERE that fixed-size window sits rather than taking a bounding
# box of all motion — a bounding box is not stable: on one run a terminal repainting just
# outside the window dragged the top edge up by 45 px and put a strip of title bar into
# all 38 clips. The size is then capped at the client window's own dimensions for the
# same reason.
cap_detect_canvas() {
  local repo=$1 win=$2
  local SX=999999 SY=999999 SR=0 SB=0 w
  for w in $(xdotool search --name '^Fosfora$'); do
    eval "$(xdotool getwindowgeometry --shell "$w" 2>/dev/null)" || continue
    (( X  < SX )) && SX=$X
    (( Y  < SY )) && SY=$Y
    (( X + WIDTH  > SR )) && SR=$((X + WIDTH))
    (( Y + HEIGHT > SB )) && SB=$((Y + HEIGHT))
  done
  log "searching for canvas within ${SX},${SY} $((SR-SX))x$((SB-SY))"

  eval "$(xdotool getwindowgeometry --shell "$win")"
  local cx cy cw ch
  read -r cx cy cw ch < <("$repo/scripts/capture/find_canvas.py" \
                            --region "$SX" "$SY" "$((SR-SX))" "$((SB-SY))" \
                            --size "$WIDTH" "$HEIGHT" --display "$DISPLAY") \
    || die "could not locate the canvas (nothing on screen was animating)"

  eval "$(xdotool getwindowgeometry --shell "$win")"
  (( cw > WIDTH  )) && cw=$WIDTH
  (( ch > HEIGHT )) && ch=$HEIGHT
  cw=$((cw - cw % 2)); ch=$((ch - ch % 2))
  [[ $cw -ge 640 && $ch -ge 360 ]] || die "detected canvas ${cw}x${ch} is implausibly small"
  # The newline is load-bearing: the caller reads this with `read`, which returns
  # non-zero when it hits EOF without a delimiter even though it did set the variables.
  # Under `set -e` that killed the whole run immediately after canvas detection, with
  # the geometry successfully found and nothing to show for it.
  printf '%s %s %s %s\n' "$cx" "$cy" "$cw" "$ch"
}

# cap_region_check <out.png> <x> <y> <w> <h>
#
# A still of exactly what will be filmed, so a bad region is caught here rather than
# discovered minutes later in every clip.
cap_region_check() {
  local out=$1 x=$2 y=$3 w=$4 h=$5
  ffmpeg -hide_banner -loglevel error -y -f x11grab -draw_mouse 0 -video_size "${w}x${h}" \
         -i "$DISPLAY+$x,$y" -frames:v 1 "$out" </dev/null
  log "wrote region check: $out"
}

# --------------------------------------------------------------------- clock

cap_now() { date +%s.%N; }

# cap_wait_until <play_t0> <seconds since play_t0>
#
# Schedule every step against an ABSOLUTE clock rather than sleeping a fixed amount per
# item. capture.sh's first version slept a fixed `LOOP_SECS*SETTLE_FRAC - 1.2`, making
# the cadence 37.5s against a 38.7s loop — a 1.2s slip per effect, 45.6s across 38,
# more than a whole loop. The phase lock the whole design rests on quietly did not
# exist: each clip landed on a different part of the music and late ones missed the drop
# entirely. Anchoring each boundary to T0 + k*LOOP absorbs whatever the per-iteration
# overhead is. T0 is PLAYBACK start, not "now" — that is what ties the schedule to the
# music.
cap_wait_until() {
  local t0=$1 at=$2 d
  d=$(python3 -c "print(max(0.0, $t0 + $at - $(cap_now)))")
  python3 -c "
import sys
if $d <= 0.0: sys.stderr.write('  [warn] behind schedule, phase lock lost\n')"
  sleep "$d"
}

# cap_first_pass <play_t0> <loop_secs> -> pass index on stdout
#
# Skip whole passes until the next slot is safely in the future — setup has already
# consumed some of the timeline.
cap_first_pass() {
  python3 -c "
import math
elapsed = $(cap_now) - $1
print(max(1, math.ceil((elapsed + 3.0) / $2)))"
}
