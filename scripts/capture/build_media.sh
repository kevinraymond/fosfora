#!/usr/bin/env bash
# Turn the raw capture clips into the media the README and gallery actually ship.
#
# Produces, from a directory of 1920x1080 clips written by capture.sh:
#   tiles/<slug>.webp   animated gallery tiles, 480x270
#   hero.webp           beat-cut montage for the top of the README
#   contact.png         one sheet of every first frame, for eyeballing the whole set at once
#
# Animated WebP rather than GIF: GitHub renders it in Markdown exactly the same way, and it is
# roughly a quarter the size at the same quality. gifski is not installed here; ffmpeg's
# palettegen/paletteuse path is the GIF fallback if one is ever needed.
#
# Every clip is phase-locked to the same audio loop, so the drop lands at the same timestamp in
# all of them. That offset is detected once (biggest frame-brightness jump, which is what a drop
# looks like in almost every effect) and then applied to the whole set, so each tile shows the
# same musical moment.
#
# Usage:  scripts/capture/build_media.sh -i CLIPDIR [-o OUTDIR] [--at SECONDS] [--hero a,b,c]

set -Eeuo pipefail

IN=  OUT=  AT=  BARS=2  BPM=124
HERO_LIST=murmur,tide,splat,polycephalum,chaos,frost
TILE_W=480
HERO_W=960
BUDGET_KB=400
HERO_BUDGET_KB=5200
ONLY=all

while [[ $# -gt 0 ]]; do
  case $1 in
    -i|--in)   IN=$2; shift 2 ;;
    -o|--out)  OUT=$2; shift 2 ;;
    --at)      AT=$2; shift 2 ;;
    --bars)    BARS=$2; shift 2 ;;
    --hero)    HERO_LIST=$2; shift 2 ;;
    --budget)  BUDGET_KB=$2; shift 2 ;;
    --only)    ONLY=$2; shift 2 ;;   # tiles | hero | all — iterate on one without redoing the rest
    -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[[ -n $IN ]] || { echo "need -i CLIPDIR" >&2; exit 2; }
OUT=${OUT:-$IN/media}

log() { printf '\033[36m[media]\033[0m %s\n' "$*" >&2; }

mkdir -p "$OUT/tiles"

# Two bars at the loop tempo: tiles loop on a musical boundary rather than mid-phrase.
DUR=$(python3 -c "print(f'{$BARS*4*60/$BPM:.4f}')")

# ---------------------------------------------------------------- find the drop
# Sample mean brightness once per 0.25 s and take the largest rise. Averaged over several
# clips so one effect that happens to darken on the drop cannot skew the choice.
if [[ -z $AT ]]; then
  log "detecting the drop offset…"
  AT=$(python3 - "$IN" <<'PY'
import json, subprocess, sys, pathlib, statistics

clips = sorted(pathlib.Path(sys.argv[1]).glob("*.mp4"))
probes = [c for c in clips if c.stem in
          ("aurora", "tunnel", "pulse", "cascade", "array", "prism", "storm", "shards")] or clips[:8]

votes = []
for c in probes:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-f", "lavfi",
         f"movie={c},fps=4,signalstats", "-show_entries",
         "frame=pkt_pts_time:frame_tags=lavfi.signalstats.YAVG",
         "-of", "json"], capture_output=True, text=True).stdout
    try:
        frames = json.loads(out).get("frames", [])
    except json.JSONDecodeError:
        continue
    ys = [(i * 0.25, float(f["tags"]["lavfi.signalstats.YAVG"]))
          for i, f in enumerate(frames) if "tags" in f]
    if len(ys) < 20:
        continue
    # Largest rise across a 1 s window, ignoring the first/last second.
    best_t, best_d = None, 0.0
    for i in range(4, len(ys) - 8):
        d = ys[i + 4][1] - ys[i][1]
        if d > best_d:
            best_d, best_t = d, ys[i][0]
    if best_t is not None and best_d > 1.0:
        votes.append(best_t)

print(f"{statistics.median(votes):.2f}" if votes else "9.9")
PY
)
fi
log "using drop offset ${AT}s, ${BARS} bars (${DUR}s) per tile"

# ---------------------------------------------------------------- gallery tiles
shopt -s nullglob
CLIPS=("$IN"/*.mp4)
if [[ $ONLY == all || $ONLY == tiles ]]; then
log "building ${#CLIPS[@]} tiles (budget ${BUDGET_KB}KB each)…"

# Encode at full quality, then step down only for the tiles that blow the budget. Effects
# differ enormously in how well they compress — Turing is near-random black-and-white speckle
# and lands ~8x the size of Murmur at identical settings — so a single global quality either
# bloats the page or needlessly softens the 30 tiles that were never the problem.
#
# The ladder shrinks WIDTH first and quality only gently. An earlier version held 480px and
# dropped quality to q=8, which visibly wrecked colour — Prism came out magenta-and-green where
# the source is cream-and-cyan. Tiles render about 300px wide in a three-column table, so 480 is
# already oversampled and losing width costs nothing visible; losing chroma precision does.
LADDER=("480 24 62" "440 22 55" "400 20 48" "360 18 42" "320 16 36" "300 14 30")

for c in "${CLIPS[@]}"; do
  slug=$(basename "$c" .mp4)
  dst=$OUT/tiles/$slug.webp
  for rung in "${LADDER[@]}"; do
    read -r tw fps q <<<"$rung"
    ffmpeg -hide_banner -loglevel error -y -ss "$AT" -t "$DUR" -i "$c" \
      -vf "fps=$fps,scale=${tw}:-2:flags=lanczos" \
      -c:v libwebp_anim -lossless 0 -q:v "$q" -compression_level 6 -loop 0 -an "$dst"
    kb=$(( $(stat -c%s "$dst") / 1024 ))
    (( kb <= BUDGET_KB )) && break
  done
  note=""; (( kb > BUDGET_KB )) && note="  (over budget even at the floor)"
  printf '  %-22s %5sKB  %sx fps=%s q=%s%s\n' "$slug" "$kb" "$tw" "$fps" "$q" "$note" >&2
done

# ---------------------------------------------------------------- contact sheet
log "building contact sheet…"
i=0
rm -rf "$OUT/.frames"; mkdir -p "$OUT/.frames"
for c in "${CLIPS[@]}"; do
  slug=$(basename "$c" .mp4)
  ffmpeg -hide_banner -loglevel error -y -ss "$AT" -i "$c" -frames:v 1 \
    -vf "scale=384:216,drawtext=text='$slug':x=6:y=6:fontsize=16:fontcolor=white:box=1:boxcolor=black@0.6:boxborderw=3" \
    "$(printf '%s/.frames/%03d.png' "$OUT" $i)"
  i=$((i+1))
done
ffmpeg -hide_banner -loglevel error -y -pattern_type glob -i "$OUT/.frames/*.png" \
  -filter_complex "tile=5x8:margin=4:padding=4" -frames:v 1 "$OUT/contact.png"
rm -rf "$OUT/.frames"
log "contact sheet: $OUT/contact.png"
fi   # ONLY == all|tiles

# ---------------------------------------------------------------- hero montage
if [[ $ONLY == all || $ONLY == hero ]]; then
# Hard cuts on a bar line — a crossfade would blur exactly the moment each effect is meant to
# announce itself.
log "building hero from: $HERO_LIST"
# One bar per cut. Two bars each looked better in isolation but pushed a six-effect montage past
# 20 s, and a hero that large is a slow first impression on a phone.
HERO_DUR=$(python3 -c "print(f'{1*4*60/$BPM:.4f}')")
args=() ; filt=() ; n=0
IFS=',' read -ra HERO <<<"$HERO_LIST"
for slug in "${HERO[@]}"; do
  f=$IN/$slug.mp4
  [[ -f $f ]] || { log "  skip $slug (no clip)"; continue; }
  args+=(-ss "$AT" -t "$HERO_DUR" -i "$f")
  filt+=("[$n:v]fps=24,scale=${HERO_W}:-2:flags=lanczos,setpts=PTS-STARTPTS[v$n];")
  n=$((n+1))
done
(( n > 0 )) || { log "no hero clips found"; exit 1; }
concat=""; for ((j=0;j<n;j++)); do concat+="[v$j]"; done
# Budget the hero too — it is the first thing that loads on the page, and 960px/24fps/q66 came
# out at 6.6 MB. Same principle as the tiles: give up width before giving up colour.
# Quality-first ladder. The hero is one image on the page, so a megabyte matters far less
# than looking good: at q=50 Murmur's fine particle grain mushed into visible blocks against
# its smooth twilight gradient, which is exactly the content lossy WebP handles worst.
for rung in "800 20 80" "720 20 74" "720 18 68" "640 18 62" "576 16 56"; do
  read -r hw hfps hq <<<"$rung"
  filt=(); for ((j=0;j<n;j++)); do
    filt+=("[$j:v]fps=$hfps,scale=${hw}:-2:flags=lanczos,setpts=PTS-STARTPTS[v$j];")
  done
  ffmpeg -hide_banner -loglevel error -y "${args[@]}" \
    -filter_complex "${filt[*]}${concat}concat=n=$n:v=1:a=0[out]" -map "[out]" \
    -c:v libwebp_anim -lossless 0 -q:v "$hq" -compression_level 6 -loop 0 -an \
    "$OUT/hero.webp"
  hkb=$(( $(stat -c%s "$OUT/hero.webp") / 1024 ))
  (( hkb <= HERO_BUDGET_KB )) && break
done
log "hero: ${hkb}KB  ${hw}px fps=$hfps q=$hq  ($n cuts, ${HERO_DUR}s each)"

fi   # ONLY == all|hero

log "done — $OUT"
