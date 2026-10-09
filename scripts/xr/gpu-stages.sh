#!/usr/bin/env bash
# GPU render-stage sweep on a Quest over adb (board #3796, #3798): for each
# condition, relaunch the installed APK with that condition's
# debug.fosfora.* knobs, let it warm up, then take one second of
# ovrgpuprofiler's render-stage trace (per compute dispatch and per render
# pass: Binning, Render, StoreColor, Preempt in ms) plus a short VrApi
# capture, and print one line per condition: the sim dispatch (the largest
# compute of each frame, median and p90), the eye surface's binning and
# render, the faces surface's render, App med, GPU MHz, particles alive.
# See docs/xr/QUEST_DOCS.md for the tool; detailed profiling adds ~10 % GPU.
#
#   scripts/xr/gpu-stages.sh [--seconds 25] [--with "music=1;gov=0"]
#                            [--only "base,count200"] [--out DIR]
#                            [--cond "name=knob=v;knob=v" ...]
#
# Runs unworn or worn alike (unworn needs the headless setup first:
# prox_close + guardian_pause + anchors replay, as worn-ab.sh does not do it
# either). Default conditions below; --cond adds more.
set -euo pipefail
cd "$(dirname "$0")/../.."
PACKAGE=dev.fosfora.xr
ACTIVITY=android.app.NativeActivity
seconds=25; with="music=1;gov=0"; only=""; out=""
conditions=("base|-" "base2|-")
while [ $# -gt 0 ]; do
    case "$1" in
        --seconds) seconds="$2"; shift 2 ;;
        --with) with="$2"; shift 2 ;;
        --only) only="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --cond) conditions+=("${2%%=*}|${2#*=}"); shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done
out="${out:-/tmp/gpu-stages-$(date +%H%M)}"; mkdir -p "$out"
all_knobs=""
for c in "${conditions[@]}"; do k="${c#*|}"; [ "$k" = "-" ] && continue; for kv in ${k//;/ }; do all_knobs="$all_knobs ${kv%%=*}"; done; done
for kv in ${with//;/ }; do all_knobs="$all_knobs ${kv%%=*}"; done
clear_knobs() { for k in $all_knobs; do adb shell setprop "debug.fosfora.$k" '""'; done; }
trap 'clear_knobs; adb shell am force-stop $PACKAGE; adb shell ovrgpuprofiler -d >/dev/null 2>&1 || true' EXIT
adb shell "ovrgpuprofiler -e $PACKAGE" >/dev/null
cat > "$out/parse.py" <<'PY'
import re, sys
groups, cur, eyes, faces = [], [], [], []
for l in open(sys.argv[1]):
    m = re.match(r'(Compute|Surface) \d+\s+\|(?:\s*(\d+)\s*x\s*(\d+)\s*\|)?.*?\|\s*([\d.]+) ms \|(.*)$', l)
    if not m:
        continue
    tail = m.group(5)
    d = re.search(r'Dispatch : ([\d.]+)ms', tail); b = re.search(r'Binning : ([\d.]+)ms', tail); r = re.search(r'Render : ([\d.]+)ms', tail)
    if m.group(1) == 'Compute':
        cur.append(float(d.group(1)) if d else 0.0); continue
    if cur:
        groups.append(cur); cur = []
    w = int(m.group(2) or 0)
    (eyes if w >= 1200 else faces).append((float(b.group(1)) if b else 0.0, float(r.group(1)) if r else 0.0))
def med(a): a = sorted(a); return a[len(a)//2] if a else float('nan')
def p90(a): a = sorted(a); return a[int(len(a)*0.9)] if a else float('nan')
sims = [max(g) for g in groups]
print(f"sim {med(sims):.2f}/{p90(sims):.2f} eye_bin {med([e[0] for e in eyes]):.2f} eye_render {med([e[1] for e in eyes]):.2f} faces_render {med([f[1] for f in faces]):.2f} frames {len(sims)}")
PY
printf "%-10s %-12s %-8s %-10s %-12s %-7s %-8s %-7s\n" cond sim_med/p90 eye_bin eye_render faces_render app mhz alive | tee "$out/summary.txt"
for entry in "${conditions[@]}"; do
    name="${entry%%|*}"; knobs="${entry#*|}"
    if [ -n "$only" ] && ! [[ ",$only," == *",$name,"* ]]; then continue; fi
    adb shell am force-stop $PACKAGE; clear_knobs
    for kv in ${with//;/ }; do adb shell "setprop debug.fosfora.${kv%%=*} '${kv#*=}'"; done
    if [ "$knobs" != "-" ]; then for kv in ${knobs//;/ }; do adb shell "setprop debug.fosfora.${kv%%=*} '${kv#*=}'"; done; fi
    adb logcat -c
    adb shell am start -n "$PACKAGE/$ACTIVITY" >/dev/null
    sleep "$seconds"
    adb shell "ovrgpuprofiler -t1.0 -v" > "$out/$name.trace" 2>&1
    timeout 10 adb logcat -v time VrApi:I fosfora_xr:I '*:S' > "$out/$name.log" 2>/dev/null </dev/null || true
    app=$(grep -oE 'App=[0-9.]+' "$out/$name.log" | cut -d= -f2 | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}')
    mhz=$(grep -oE '/[0-9]+MHz' "$out/$name.log" | tail -1 | tr -d /)
    alive=$(grep -oE 'particles alive [0-9]+' "$out/$name.log" | tail -1 | awk '{print $3}')
    line="$name $(python3 -I "$out/parse.py" "$out/$name.trace") app $app $mhz alive $alive"
    echo "$line" >> "$out/summary.txt"; echo "$line"
done
echo "traces in $out" >&2
