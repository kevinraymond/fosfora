#!/usr/bin/env bash
# Worn A/B of the frame cost on a Quest over adb (board #3783): the wearer
# keeps the headset on, standing in one place with both hands in view at
# chest height, looking at the desk, no menu, no speech; this script
# relaunches the installed APK once per condition with that condition's
# debug.fosfora.* knobs, lets it warm up, then captures the runtime's
# per-second frame line for --seconds and summarizes App GPU ms (median,
# p90, max), the compositor's TW ms (median, p90: a second composition
# layer costs there, not in App, board #3793), fps, stale frames, GPU MHz
# and the runtime's temperature. The
# full build runs first and last, so drift inside the sitting shows.
#
#   scripts/xr/worn-ab.sh [--seconds 60] [--warmup 20] [--out DIR]
#                         [--only "full,cloud0,..."] [--with "music=1;..."]
#
# --with sets extra knobs for every condition (music=1 plays the bundled
# clip, which the room's emitters need: a quiet room emits nothing and the
# cloud is empty, board #3783; gov=0 holds the thermal governor, board
# #3789, which otherwise thins the cloud during a run that is over budget
# and makes every condition read the same).
#
# Every knob is cleared afterwards. The surface knob writes the room file
# (rooms/<id>.json under the config dir), so the room files are backed up
# first and put back after each surface condition and at the end; the
# cloud knob saves nothing. Run after a reboot; note whether the headset
# is charging.
set -euo pipefail
cd "$(dirname "$0")/../.."

PACKAGE=dev.fosfora.xr
ACTIVITY=android.app.NativeActivity
seconds=60
warmup=20
out=""
only=""
with=""
while [ $# -gt 0 ]; do
    case "$1" in
        --seconds) seconds="$2"; shift 2 ;;
        --warmup) warmup="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --only) only="$2"; shift 2 ;;
        --with) with="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done
out="${out:-/tmp/worn-ab-$(date +%H%M)}"
mkdir -p "$out"

NONE='table=none,wall=none,floor=none,ceiling=none,frame=none,other=none'
# name | knobs (name=value;...) | what it removes
conditions=(
    "full|-|everything on: the cloud, the lit surfaces, the depth occluder and collision, the hand mesh"
    "cloud0|cloud=0|the cloud hidden (its sim still steps)"
    "surf0|surface=$NONE;canvas=0;ripple=0|every surface to none, no wall spectrum, no rings"
    "depth0|envdepth=0|no environment depth: no occluder, no depth collision"
    "mesh0|handmesh=0|hand joint spheres as the occluder instead of the skinned mesh"
    "hands0|hands=0;handmesh=0|no hand obstacles and no hand occluder"
    "bare|cloud=0;surface=$NONE;canvas=0;ripple=0;envdepth=0|the cloud, the surfaces and the depth off together"
    "ported0|surface=wall=none,floor=none,ceiling=none,frame=none|the ported face effects and streamlines off, the embers (and so the cloud) kept"
    "layer0|faceslayer=0|the surfaces back in the eye pass"
    "layer0b|faceslayer=0|the surfaces back in the eye pass, again (interleaved with full2 against drift)"
    "scale25|facescale=0.25|the faces layer at a quarter of the recommended size"
    "cheap|surface=wall=pulse,floor=rings|the same walls and floor on the built-in pulse and rings instead of the ported effects"
    "full2|-|everything on again (drift check)"
)

all_knobs="cloud surface canvas ripple envdepth handmesh hands faceslayer facescale"
for kv in ${with//;/ }; do all_knobs="$all_knobs ${kv%%=*}"; done
clear_knobs() {
    for k in $all_knobs; do adb shell setprop "debug.fosfora.$k" '""'; done
}
# The room files, byte for byte, through the app's own user.
rooms=$(adb shell "run-as $PACKAGE sh -c 'ls files/config/rooms/'" | tr -d '\r')
mkdir -p "$out/rooms"
for r in $rooms; do
    adb exec-out "run-as $PACKAGE cat files/config/rooms/$r" > "$out/rooms/$r"
done
restore_rooms() {
    adb shell am force-stop $PACKAGE
    for r in $rooms; do
        adb push "$out/rooms/$r" "/data/local/tmp/$r" >/dev/null
        adb shell "run-as $PACKAGE sh -c 'cp /data/local/tmp/$r files/config/rooms/$r'"
        adb shell rm "/data/local/tmp/$r"
    done
    adb shell "run-as $PACKAGE sh -c 'md5sum files/config/rooms/*'" | sed 's/^/room now: /' >&2
}
md5sum "$out"/rooms/* | sed 's/^/room was: /' >&2
trap 'clear_knobs; restore_rooms' EXIT

stats() {
    # App GPU ms med/p90/max, compositor TW ms med/p90, fps med, stale/s
    # med, GPU MHz mode, temp max
    # (plain awk: no asort; the sorts are insertion sorts over ~60 rows).
    awk '
    function sortn(arr, n,   i, j, v) {
        for (i = 2; i <= n; i++) { v = arr[i]; j = i - 1; while (j > 0 && arr[j] > v) { arr[j+1] = arr[j]; j-- } arr[j+1] = v }
    }
    /FPS=/ {
        n++
        match($0, /App=[0-9.]+/); a[n] = substr($0, RSTART+4, RLENGTH-4) + 0
        match($0, /TW=[0-9.]+/); w[n] = substr($0, RSTART+3, RLENGTH-3) + 0
        match($0, /FPS=[0-9]+/); f[n] = substr($0, RSTART+4, RLENGTH-4) + 0
        match($0, /Stale=[0-9]+/); s[n] = substr($0, RSTART+6, RLENGTH-6) + 0
        match($0, /\/[0-9]+MHz/); m = substr($0, RSTART+1, RLENGTH-4); mhz[m]++
        match($0, /Temp=[0-9.]+/); t = substr($0, RSTART+5, RLENGTH-5) + 0; if (t > tmax) tmax = t
    }
    END {
        if (n == 0) { print "no frame lines"; exit }
        sortn(a, n); sortn(w, n); sortn(f, n); sortn(s, n)
        best = ""; bc = 0; for (k in mhz) if (mhz[k] > bc) { best = k; bc = mhz[k] }
        mid = int((n + 1) / 2); p90 = int(n * 0.9); if (p90 < 1) p90 = 1
        printf "app_med=%.2f app_p90=%.2f app_max=%.2f tw_med=%.2f tw_p90=%.2f fps_med=%d stale_med=%d gpu_mhz=%s temp_max=%.0f n=%d\n", a[mid], a[p90], a[n], w[mid], w[p90], f[mid], s[mid], best, tmax, n
    }' "$1"
}

printf "%-7s %-10s %-9s %-8s %-8s %-8s %-8s %-10s %-8s %-9s\n" cond app_med app_p90 app_max tw_med tw_p90 fps_med stale_med gpu_mhz temp_max | tee "$out/summary.txt"
for entry in "${conditions[@]}"; do
    IFS='|' read -r name knobs what <<<"$entry"
    if [ -n "$only" ] && ! [[ ",$only," == *",$name,"* ]]; then continue; fi
    adb shell am force-stop $PACKAGE
    clear_knobs
    for kv in ${with//;/ }; do
        adb shell "setprop debug.fosfora.${kv%%=*} '${kv#*=}'"
    done
    if [ "$knobs" != "-" ]; then
        for kv in ${knobs//;/ }; do
            adb shell "setprop debug.fosfora.${kv%%=*} '${kv#*=}'"
        done
    fi
    echo "== $name: $what (warmup ${warmup}s, run ${seconds}s)" >&2
    adb logcat -c
    adb shell am start -n "$PACKAGE/$ACTIVITY" >/dev/null
    sleep "$warmup"
    adb logcat -c
    timeout "$seconds" adb logcat -v time fosfora_xr:I VrApi:I '*:S' > "$out/$name.log" 2>/dev/null </dev/null || true
    grep -oE 'particles alive [0-9]+ · emitter weight [0-9.]+' "$out/$name.log" | tail -1 >&2 || true
    line=$(stats "$out/$name.log")
    echo "$name $line" >> "$out/summary.txt"
    printf "%-7s %s\n" "$name" "$line"
    case "$knobs" in *surface=*) restore_rooms ;; esac
done
clear_knobs
echo "logs in $out" >&2
