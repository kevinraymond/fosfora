#!/usr/bin/env bash
# Board #4163 round (the beat activation into the phase arbiter): run one arm end to end.
#
#   bench/q4163_arm.sh <arm> <binary> [VAR=value ...]
#
# Into /mnt/t9/fosfora-work/q4163/bench/<arm>/ (its own root: the bench keys its dump cache
# on the binary hash, so arms that differ only by an environment knob must not share one):
#   results/<dataset>/, dumps/<dataset>/   run_bench on ballroom, harmonix, giantsteps_tempo
#                                          (both halves dumped; only the tune half is read
#                                          before the verdict)
#   batida/<render>.analysis.json, batida.jsonl   --analyze on the 80 batida renders, scored
#                                          by batida's score_fosfora.py (exact labels)
#   negatives/<id>.jsonl                   --signal-dump --feat-bus on the 155 beatless clips
# The extra VAR=value arguments are exported for every binary invocation (the round's knobs).
set -euo pipefail
arm=$1
bin=$(readlink -f "$2")
shift 2
REPO=$(cd "$(dirname "$0")/.." && pwd)
ROOT=/mnt/t9/fosfora-work/q4163/bench/$arm
REN=/mnt/t9/fosfora-work/q4163/data/batida/renders
NEG=$HOME/ai/audio/kick-work/fma/negatives/clips
export FOSFORA_BIN=$bin
for kv in "$@"; do export "$kv"; done
mkdir -p "$ROOT/batida" "$ROOT/negatives"
{
  echo "arm $arm binary $bin sha256 $(sha256sum "$bin" | cut -c1-16) knobs: $*"
  date -u +%FT%TZ
} > "$ROOT/arm.txt"

cd "$REPO"
for ds in ballroom harmonix giantsteps_tempo; do
  nice -n 15 uv run --quiet --script bench/run_bench.py "bench/datasets/$ds" --jobs 8 --out-root "$ROOT" > "$ROOT/run_$ds.log" 2>&1 &
done
wait
tail -n1 "$ROOT"/run_*.log

pairs=()
for wav in "$REN"/*.wav; do
  n=$(basename "$wav" .wav)
  [ -f "$REN/$n.json" ] || continue
  if [ ! -f "$ROOT/batida/$n.analysis.json" ]; then
    nice -n 19 "$bin" --analyze "$wav" --out "$ROOT/batida/$n.analysis.json" > /dev/null 2> "$ROOT/batida/$n.log" &
  fi
  pairs+=("$REN/$n.json" "$ROOT/batida/$n.analysis.json")
  while [ "$(jobs -rp | wc -l)" -ge 8 ]; do wait -n; done
done
wait
python3 "$HOME/ai/audio/batida/scripts/score_fosfora.py" "${pairs[@]}" > "$ROOT/batida.jsonl"
echo "batida: $(wc -l < "$ROOT/batida.jsonl") renders scored"

for mp3 in "$NEG"/*.mp3; do
  n=$(basename "$mp3" .mp3)
  [ -f "$ROOT/negatives/$n.jsonl" ] && continue
  nice -n 19 "$bin" --signal-dump "$mp3" --out "$ROOT/negatives/$n.jsonl" --feat-bus > /dev/null 2>&1 &
  while [ "$(jobs -rp | wc -l)" -ge 8 ]; do wait -n; done
done
wait
echo "negatives: $(ls "$ROOT/negatives"/*.jsonl | wc -l) clips dumped"
date -u +%FT%TZ >> "$ROOT/arm.txt"
echo "arm $arm done"
