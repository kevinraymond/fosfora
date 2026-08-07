"""Stem-proxy validation vs ground-truth stems (MUSDB18-HQ).

Mapping is honest about what the proxies are: drums -> drums.wav,
bass -> bass.wav, melody -> vocals + other summed (the proxy is HPSS harmonic
energy — the harmonic residual, not any single stem).

Ground-truth envelope: the engine's exact mono fold ((L+R)/2), non-overlapping
512-sample frames -> RMS dB, sample-held onto the dump's decimated grid
(frame = round(ts * hop_hz) - 1; ts is hop END). Correlation only — Pearson on
comparable dB plus Spearman for monotone-robustness — because the proxies are
causally, adaptively normalized: their scale belongs to the normalizer, and
the honest claim is "moves with the true stem's loudness". Frames where every
stem is below -60 dBFS are excluded: joint silence inflates correlation and
carries no information.
"""

from __future__ import annotations

import numpy as np
from scipy import stats

from .. import dump as dump_mod
from . import CONVENTIONS, register

_SILENCE_DB = CONVENTIONS["stems"]["joint_silence_floor_dbfs"]
_HOP = 512

# wire stem name -> ground-truth stem files to sum
_MAPPING = {
    "drums": ("drums",),
    "bass": ("bass",),
    "melody": ("vocals", "other"),
}


def _mono(path) -> tuple[np.ndarray, int]:
    import soundfile as sf

    data, sr = sf.read(path, dtype="float64", always_2d=True)
    return data[:, :2].mean(axis=1) if data.shape[1] > 1 else data[:, 0], sr


def _envelope_db(mono: np.ndarray) -> np.ndarray:
    n = len(mono) // _HOP
    frames = mono[: n * _HOP].reshape(n, _HOP)
    rms = np.sqrt((frames**2).mean(axis=1))
    return 20.0 * np.log10(rms + 1e-8)


def _grid_indices(ts: np.ndarray, hop_hz: float, n_frames: int) -> np.ndarray:
    idx = np.rint(ts * hop_hz).astype(int) - 1  # ts is the hop END
    return np.clip(idx, 0, n_frames - 1)


@register("stems", ("stems",))
def stems(dump, ann) -> dict:
    gt_db: dict[str, np.ndarray] = {}
    for name, parts in _MAPPING.items():
        paths = [ann.stems.get(p) for p in parts]
        if any(p is None for p in paths):
            continue
        monos = []
        for p in paths:
            m, _sr = _mono(ann.base_dir / p)
            monos.append(m)
        n = min(len(m) for m in monos)
        gt_db[name] = _envelope_db(sum(m[:n] for m in monos))
    if not gt_db:
        return {"no_ground_truth": True}

    n_frames = min(len(e) for e in gt_db.values())
    out: dict = {}
    # All three proxies share one tick grid (same emit_continuous call), so the
    # joint-silence mask is computed once on the first stem's timestamps.
    mask = None
    for name in gt_db:
        ts, proxy = dump.series(dump_mod.STEM_ENERGY[name])
        idx = _grid_indices(ts, dump.hop_hz, n_frames)
        if mask is None:
            joint = np.stack([gt_db[s][:n_frames][idx] for s in gt_db])
            mask = ~np.all(joint < _SILENCE_DB, axis=0)
        gt = gt_db[name][:n_frames][idx][mask]
        pv = proxy[mask]
        block = {"n": int(mask.sum()), "n_excluded_silence": int((~mask).sum())}
        if len(gt) >= 2 and np.std(gt) > 0 and np.std(pv) > 0:
            block["pearson"] = float(stats.pearsonr(pv, gt).statistic)
            block["spearman"] = float(stats.spearmanr(pv, gt).statistic)
        else:
            block["pearson"] = None
            block["spearman"] = None
        out[name] = block
    return out
