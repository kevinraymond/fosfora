# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Why does lastBeatStanding beat Fosfora on the Harmonix holdout? Classify every track.

    bench/td_compare/gap_causes.py <dumps_root> <index.json>

<dumps_root> is compare.py's: fosfora/ and lbs_k30..32/ hold each system's dumps.
"""
import json, sys
from collections import Counter, defaultdict
import numpy as np
sys.path.insert(0, 'bench')
from benchlib.annotations import Annotations
from benchlib.dump import SignalDump
from benchlib.metrics.beats import _score
if len(sys.argv) != 3:
    sys.exit(__doc__)
D = sys.argv[1]
idx = json.load(open(sys.argv[2]))
def analyze(est, ref):
    est = est[est >= 5.0]
    if len(est) < 8:
        return dict(ratio=None, on=0.0)
    k = np.clip(np.searchsorted(ref, est) - 1, 0, len(ref) - 2)
    pos = ((est - ref[k]) / (ref[k + 1] - ref[k])) % 1.0
    ratio = np.median(np.diff(est)) / np.median(np.diff(ref))
    return dict(ratio=ratio, on=float(np.mean((pos < .125) | (pos > .875))), anti=float(np.mean(np.abs(pos - .5) < .125)))
cat = Counter(); loss = defaultdict(float); n_gap = 0
for t in idx:
    ref = np.asarray(Annotations.load(t['annotations']).beats, float)
    stem = f"harmonix__{t['track']}.jsonl"
    fo = np.asarray(SignalDump.load(f'{D}/fosfora/{stem}').beats(), float)
    f_fo = _score(fo, ref)['f_measure']
    f_lb = np.mean([_score(np.asarray(SignalDump.load(f'{D}/lbs_k{k}/{stem}').beats(), float), ref)['f_measure'] for k in (30, 31, 32)])
    gap = f_lb - f_fo
    if gap <= 0.1:
        cat['fosfora within .1 or better'] += 1
        continue
    n_gap += 1
    a = analyze(fo, ref)
    r = a['ratio']
    if r is None:
        c = 'fosfora emits almost nothing'
    elif abs(r - 1) > 0.04:
        c = 'tempo off: ' + ('half/double' if min(abs(r - .5), abs(r - 2)) < 0.06 else ('3:4 / 4:3 / 2:3 / 3:2' if min(abs(r - .75), abs(r - 4/3), abs(r - 2/3), abs(r - 1.5)) < 0.06 else 'other ratio'))
    elif a['on'] < 0.6:
        c = 'tempo right, phase off (anti %.0f%%)' % 0 if False else ('tempo right, mostly off-beat' if a['anti'] > a['on'] else 'tempo right, phase unstable')
    else:
        c = 'tempo right, mostly on-beat (continuity / drift)'
    cat[c] += 1; loss[c] += gap
total_gap = sum(loss.values())
print(f'tracks: {len(idx)}; lastBeatStanding better by > .1 on {n_gap}')
for c, n in cat.most_common():
    share = f'  {100 * loss[c] / total_gap:4.0f}% of the summed gap' if c in loss else ''
    print(f'  {c:52} {n:4}{share}')
