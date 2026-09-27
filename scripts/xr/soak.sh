#!/usr/bin/env bash
# S5 thermal soak on a Quest over adb: run the installed APK at one setting
# for a long time and sample, every --every seconds, the runtime's frame
# line (FPS, stale, App GPU ms, CPU/GPU MHz, its own temperature, power
# level) and Android's thermal service (throttle status, SoC / board /
# surface / battery temperatures). Throttling shows up as the GPU MHz
# dropping, App= rising and Thermal Status leaving 0.
#
#   scripts/xr/soak.sh [--count 750000] [--hz 90] [--minutes 15] [--every 15]
#                      [--out FILE]
#
# The headset can sit unworn (proximity fake + Guardian pause, board #3215).
# Note in the output whether it was charging: USB power adds heat.
set -euo pipefail

cd "$(dirname "$0")/../.."

PACKAGE=dev.fosfora.xr
ACTIVITY=android.app.NativeActivity
LOG_TAG=fosfora_xr

count=750000
hz=90
minutes=15
every=15
out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --count) count="$2"; shift 2 ;;
        --hz) hz="$2"; shift 2 ;;
        --minutes) minutes="$2"; shift 2 ;;
        --every) every="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

headless_off() {
    adb shell am force-stop "$PACKAGE"
    adb shell setprop debug.oculus.guardian_pause 0
    adb shell am broadcast -a com.oculus.vrpowermanager.automation_disable >/dev/null
}
trap headless_off EXIT

emit() {
    echo -e "$1"
    if [ -n "$out" ]; then echo -e "$1" >> "$out"; fi
}

therm() {
    # name=value pairs from the thermal service, plus its status.
    adb shell dumpsys thermalservice | python3 -c '
import re, sys
s = sys.stdin.read()
st = re.search(r"Thermal Status: (\d+)", s)
vals = dict(re.findall(r"mValue=([\d.]+), mType=\d+, mName=([\w-]+)", s)[::-1])
vals = {v: k for k, v in vals.items()}
def g(n): return vals.get(n, "-")
print("\t".join([st.group(1) if st else "-", g("soc-usr"), g("pcb1-usr"), g("surf-virt-usr"), g("batt-virt-usr")]))
'
}

charging=$(adb shell dumpsys battery | awk '/AC powered|USB powered/ {printf "%s %s ", $1, $3}')
emit "# S5 soak $(date -Is) · commit $(git rev-parse --short HEAD) · count $count · $hz Hz · $minutes min · powered: $charging"
emit "t_s\tfps\tstale\tapp_ms\tcpu_mhz\tgpu_mhz\tvrapi_temp\tpls\tthermal_status\tsoc_c\tpcb_c\tsurface_c\tbatt_c\tapp_frames_per_s\tlong_total"

adb shell am broadcast -a com.oculus.vrpowermanager.prox_close >/dev/null
adb shell setprop debug.oculus.guardian_pause 1
sleep 3
adb shell am force-stop "$PACKAGE"
adb shell setprop debug.fosfora.mode particles
adb shell setprop debug.fosfora.count "$count"
adb shell setprop debug.fosfora.hz "$hz"
adb logcat -c || true
adb shell am start -n "$PACKAGE/$ACTIVITY" >/dev/null
start=$(date +%s)
end=$((start + minutes * 60))
sleep 5
pid=$(adb shell pidof "$PACKAGE" | tr -d '\r')
while [ "$(date +%s)" -lt "$end" ]; do
    sleep "$every"
    now=$(( $(date +%s) - start ))
    line=$(adb logcat -d -v time VrApi:I '*:S' | grep "VrApi *( *$pid)" | tail -1)
    vr=$(printf '%s' "$line" | python3 -c '
import re, sys
l = sys.stdin.read()
def g(p):
    m = re.search(p, l); return m.group(1) if m else "-"
print("\t".join([g(r"FPS=(\d+)/"), g(r"Stale=(\d+)"), g(r"App=([\d.]+)ms"),
    g(r"CPU4/GPU=\d+/\d+,(\d+)/"), g(r"CPU4/GPU=\d+/\d+,\d+/(\d+)MHz"),
    g(r"Temp=([\d.]+)C"), g(r"PLS=(\d+)")]))
')
    app=$(adb logcat -d -v time "$LOG_TAG:V" '*:S' | grep "frames " | tail -1 | python3 -c '
import re, sys
l = sys.stdin.read()
m = re.search(r"frames ([\d.]+)/s", l); t = re.search(r"total (\d+)/", l)
print((m.group(1) if m else "-") + "\t" + (t.group(1) if t else "-"))
')
    emit "$now\t$vr\t$(therm)\t$app"
    adb logcat -c || true
done
