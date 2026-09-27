#!/usr/bin/env bash
# S5 particle sweep on a Quest over adb: for each (count, Hz, eye scale)
# combination, set the debug.fosfora.* knobs, relaunch the installed APK,
# let it run, then summarize the in-app pacing counters and the runtime's
# per-frame GPU time from logcat. See docs/xr/BUILD_PLAN.md (S5).
#
#   scripts/xr/sweep.sh [--counts "100000 250000 ..."] [--hz "72 90"]
#                       [--eyescale "1.0"] [--seconds 30] [--sim 1|0]
#                       [--size 1.0] [--tri 1|0] [--pull 1|0] [--out FILE]
#                       [--mode particles|mr] [--set "name=value;name=value"]
#
# --mode picks the debug.fosfora.mode the app starts in (S7 uses mr) and
# --set applies extra debug.fosfora.<name> knobs to every run (e.g.
# "passthrough=1;hands=0;room=0" for the S7 matrix); they are cleared afterwards.
#
# The headset can sit unworn: the script fakes the proximity sensor and
# pauses Guardian for the run (board #3215) and restores both afterwards.
# One line per run in TSV: count hz eyescale sim size tri pull frames/s long_frames
# total_frames cpu_avg_ms gpu_median_ms gpu_max_ms fps_runtime stale.
set -euo pipefail

cd "$(dirname "$0")/../.."

PACKAGE=dev.fosfora.xr
ACTIVITY=android.app.NativeActivity
LOG_TAG=fosfora_xr

counts="100000 250000 500000 1000000 2000000"
hzs="72 90"
eyescales="1.0"
seconds=30
sim=1
size=1.0
tri=1
pull=1
out=""
mode=particles
set_knobs=""
while [ $# -gt 0 ]; do
    case "$1" in
        --counts) counts="$2"; shift 2 ;;
        --hz) hzs="$2"; shift 2 ;;
        --eyescale) eyescales="$2"; shift 2 ;;
        --seconds) seconds="$2"; shift 2 ;;
        --sim) sim="$2"; shift 2 ;;
        --size) size="$2"; shift 2 ;;
        --tri) tri="$2"; shift 2 ;;
        --pull) pull="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --mode) mode="$2"; shift 2 ;;
        --set) set_knobs="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

headless_on() {
    adb shell am broadcast -a com.oculus.vrpowermanager.prox_close >/dev/null
    adb shell setprop debug.oculus.guardian_pause 1
    # The first launch right after the proximity fake can start paused and
    # log nothing (seen as an empty first row); give the runtime a moment.
    sleep 3
}
headless_off() {
    for kv in ${set_knobs//;/ }; do
        adb shell setprop "debug.fosfora.${kv%%=*}" '""'
    done
    adb shell setprop debug.oculus.guardian_pause 0
    adb shell am broadcast -a com.oculus.vrpowermanager.automation_disable >/dev/null
}
trap headless_off EXIT

emit() {
    echo -e "$1"
    if [ -n "$out" ]; then echo -e "$1" >> "$out"; fi
}

emit "# sweep $(date -Is) · commit $(git rev-parse --short HEAD) · ${seconds}s per run · mode $mode · set [$set_knobs]"
emit "count\thz\teyescale\tsim\tsize\ttri\tpull\tframes_per_s\tlong\ttotal\tcpu_avg_ms\tgpu_med_ms\tgpu_max_ms\truntime_fps\tstale"

headless_on
for es in $eyescales; do
for hz in $hzs; do
for count in $counts; do
    adb shell am force-stop "$PACKAGE"
    adb shell setprop debug.fosfora.mode "$mode"
    for kv in ${set_knobs//;/ }; do
        adb shell setprop "debug.fosfora.${kv%%=*}" "${kv#*=}"
    done
    adb shell setprop debug.fosfora.count "$count"
    adb shell setprop debug.fosfora.hz "$hz"
    adb shell setprop debug.fosfora.eyescale "$es"
    adb shell setprop debug.fosfora.sim "$sim"
    adb shell setprop debug.fosfora.size "$size"
    adb shell setprop debug.fosfora.tri "$tri"
    adb shell setprop debug.fosfora.pull "$pull"
    adb logcat -c || true
    adb shell am start -n "$PACKAGE/$ACTIVITY" >/dev/null
    sleep "$seconds"
    log=$(adb logcat -d -v time "$LOG_TAG:V" VrApi:I '*:S')
    adb shell am force-stop "$PACKAGE"

    # Skip the first 5 s (startup, refresh-rate switch) in every statistic.
    summary=$(printf '%s\n' "$log" | python3 -c '
import re, sys, statistics
lines = sys.stdin.read().splitlines()
frames, longs, cpu, gpu, fps, stale = [], [], [], [], [], []
total = "?"
skip = 5
# The runtime logs a VrApi line per process; keep the app line (same pid
# as the fosfora_xr lines), not the compositor one.
pid = None
for l in lines:
    m = re.search(r"I/fosfora_xr\((\s*\d+)\)", l)
    if m:
        pid = m.group(1).strip()
        break
for l in lines:
    if "VrApi" in l and pid and not re.search(r"VrApi\s*\(\s*" + pid + r"\)", l):
        continue
    m = re.search(r"frames ([\d.]+)/s .* cpu avg ([\d.]+) max [\d.]+ ms · long (\d+) \(total (\d+)/(\d+)\)", l)
    if m:
        frames.append(float(m.group(1))); cpu.append(float(m.group(2)))
        longs.append(int(m.group(3))); total = m.group(5)
        continue
    m = re.search(r"FPS=(\d+)/(\d+).*?Stale=(\d+)", l)
    g = re.search(r"App=([\d.]+)", l)
    if m and g:
        fps.append(int(m.group(1))); stale.append(int(m.group(3))); gpu.append(float(g.group(1)))
def med(v): return f"{statistics.median(v):.2f}" if v else "-"
def mx(v): return f"{max(v):.2f}" if v else "-"
frames, cpu, longs = frames[skip:], cpu[skip:], longs[skip:]
fps, stale, gpu = fps[skip:], stale[skip:], gpu[skip:]
print("\t".join([
    med(frames), str(sum(longs)) if longs else "-", total,
    med(cpu), med(gpu), mx(gpu),
    med(fps), str(sum(stale)) if stale else "-",
]))
')
    emit "$count\t$hz\t$es\t$sim\t$size\t$tri\t$pull\t$summary"
done
done
done
