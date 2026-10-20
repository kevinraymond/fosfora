#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Read a FOSFORA_HOP_SIDECAR dump into per-hop arrays.

The dump is written by `crates/fosfora-app/src/audio/hop_sidecar.rs`: a header line that
names every column, then one JSONL record per analysis hop with the RAW evidence the
detectors read on that hop (nothing normalized, smoothed or held): the kick model's input
frame and probability, the SuperFlux onset flux (combined, low, mid, high), the perceptual
silence flag and the pre-normalization `AudioFeatures` snapshot, plus the beat the grid
fired on that hop if any. This module is the only reader; it must stay in step with the
writer (`v` is the format version).

    hopdump.py <dump.jsonl>             # summary: hops, duration, beats, columns

    import hopdump
    d = hopdump.read("track.jsonl")
    X, names = d.acoustic()              # pre_norm minus the tracker's own slots
    S = hopdump.stack_context(d.kf, 2)   # this hop, then 1 and 2 hops back

Hop h is the analysis frame ending at sample (h + 1) * hop; ts = (h + 1) * hop / sr. The
writer records every hop (unlike the feature bus, whose 86 Hz tx clock skips ~1 in 650),
so `idx` is 0..H-1 with no gaps, and `read` refuses a dump where it is not.
"""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np

FORMAT_VERSION = 1

# `pn` slots that are the beat, downbeat, structure and key trackers' own outputs. In the
# snapshot the first 15 carry the PREVIOUS hop's value (hop_sidecar.rs), on pseudo-labeled
# audio they ARE the label, and `onset` is the detector's held/decayed strength (the raw
# flux is in `sf`). None of them is model input.
NOT_ACOUSTIC = frozenset(
    [
        "onset", "beat", "beat_phase", "bpm", "beat_strength",
        "downbeat", "bar_phase", "beat_in_bar",
        "section_novelty", "buildup", "drop",
        "bar_index", "beat_index", "tempo_confidence", "beat_locked",
        "key_class", "key_is_minor", "key_confidence",
    ]
)


@dataclass
class HopDump:
    path: str
    sr: float
    hop: int
    idx: np.ndarray        # int64  [H]   0..H-1
    ts: np.ndarray         # float64 [H]  sample-clock seconds
    silent: np.ndarray     # bool   [H]   the A10 perceptual-silence gate
    kick_prob: np.ndarray  # float32 [H]  kick model probability, ungated
    kf: np.ndarray         # float32 [H, 24] kick model input frame
    kf_names: list[str]
    sf: np.ndarray         # float32 [H, 4]  SuperFlux: combined, low, mid, high
    sf_names: list[str]
    pn: np.ndarray         # float32 [H, 85] pre-normalization AudioFeatures
    pn_names: list[str]
    beat_times: np.ndarray  # float64 [B] event times of the beats the grid fired

    @property
    def hop_hz(self) -> float:
        return self.sr / self.hop

    @property
    def n_hops(self) -> int:
        return len(self.idx)

    @property
    def duration(self) -> float:
        return self.n_hops * self.hop / self.sr

    def acoustic(self) -> tuple[np.ndarray, list[str]]:
        """The `pn` columns that are acoustic evidence, with their names."""
        keep = [i for i, n in enumerate(self.pn_names) if n not in NOT_ACOUSTIC]
        return self.pn[:, keep], [self.pn_names[i] for i in keep]

    def column(self, name: str) -> np.ndarray:
        """One `pn` column by name."""
        return self.pn[:, self.pn_names.index(name)]


def _floats(values, width: int, what: str, hop: int) -> np.ndarray:
    if len(values) != width:
        raise ValueError(f"hop {hop}: {what} has {len(values)} values, header says {width}")
    try:
        return np.asarray(values, dtype=np.float32)
    except (TypeError, ValueError):
        return np.asarray([np.nan if v is None else v for v in values], dtype=np.float32)


def read(path: str | Path) -> HopDump:
    path = Path(path)
    with open(path, encoding="utf-8") as f:
        first = f.readline()
        if not first:
            raise ValueError(f"{path}: empty dump")
        header = json.loads(first)
        if header.get("k") != "h":
            raise ValueError(f"{path}: first line is not a header: {first[:80]!r}")
        if header.get("v") != FORMAT_VERSION:
            raise ValueError(f"{path}: format v{header.get('v')}, this reader is v{FORMAT_VERSION}")
        kf_names, sf_names, pn_names = header["kf"], header["sf"], header["pn"]
        n_kf, n_sf, n_pn = len(kf_names), len(sf_names), len(pn_names)

        idx, ts, silent, kp, beats = [], [], [], [], []
        kf, sf, pn = [], [], []
        for n, line in enumerate(f):
            if not line.strip():
                continue
            r = json.loads(line)
            if r.get("k") != "r":
                raise ValueError(f"{path} line {n + 2}: not a record: {line[:80]!r}")
            if r["i"] != n:
                raise ValueError(f"{path}: record {n} carries i={r['i']}; hops must be contiguous")
            idx.append(r["i"])
            ts.append(r["ts"])
            silent.append(bool(r["sil"]))
            kp.append(np.nan if r["kp"] is None else r["kp"])
            kf.append(_floats(r["kf"], n_kf, "kf", n))
            sf.append(_floats(r["sf"], n_sf, "sf", n))
            pn.append(_floats(r["pn"], n_pn, "pn", n))
            if r["bt"] is not None:
                beats.append(r["bt"])

    h = len(idx)
    return HopDump(
        path=str(path),
        sr=float(header["sr"]),
        hop=int(header["hop"]),
        idx=np.asarray(idx, dtype=np.int64),
        ts=np.asarray(ts, dtype=np.float64),
        silent=np.asarray(silent, dtype=bool),
        kick_prob=np.asarray(kp, dtype=np.float32),
        kf=np.vstack(kf) if h else np.zeros((0, n_kf), np.float32),
        kf_names=list(kf_names),
        sf=np.vstack(sf) if h else np.zeros((0, n_sf), np.float32),
        sf_names=list(sf_names),
        pn=np.vstack(pn) if h else np.zeros((0, n_pn), np.float32),
        pn_names=list(pn_names),
        beat_times=np.asarray(beats, dtype=np.float64),
    )


def stack_context(F: np.ndarray, n_back: int) -> np.ndarray:
    """[H, (n_back + 1) * D]: each hop's row followed by the rows 1, 2, ... n_back hops
    back, zeros before the start. The order `kick_model.rs` builds its ensemble input in
    (this frame first, then `history[0]` = one back, `history[1]` = two back), so a model
    trained on this layout reads the runtime's vector unchanged."""
    F = np.asarray(F)
    if F.ndim != 2:
        raise ValueError(f"expected [H, D], got shape {F.shape}")
    h, d = F.shape
    cols = [F]
    for k in range(1, n_back + 1):
        if k >= h:
            cols.append(np.zeros_like(F))
        else:
            cols.append(np.vstack([np.zeros((k, d), F.dtype), F[:-k]]))
    return np.hstack(cols)


def summary(d: HopDump) -> str:
    kick = d.column("kick")
    lines = [
        f"{d.path}",
        f"  sr {d.sr:g} Hz, hop {d.hop} ({d.hop_hz:.4f} hops/s), {d.n_hops} hops = {d.duration:.1f} s",
        f"  silent hops {int(d.silent.sum())} ({100 * d.silent.mean():.1f} %)" if d.n_hops else "  no hops",
        f"  beats fired {len(d.beat_times)}",
        f"  kick_prob mean {np.nanmean(d.kick_prob):.3f}, kick >= .5 on {int((kick >= 0.5).sum())} hops",
        f"  sf combined mean {np.nanmean(d.sf[:, 0]):.4f}, > 0 on {int((d.sf[:, 0] > 0).sum())} hops",
        f"  columns: kf {len(d.kf_names)}, sf {len(d.sf_names)}, pn {len(d.pn_names)} "
        f"(acoustic {len(d.acoustic()[1])})",
    ]
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    if len(argv) != 2 or argv[1] in ("-h", "--help"):
        print(__doc__.strip().splitlines()[0])
        print("usage: hopdump.py <dump.jsonl>")
        return 2
    print(summary(read(argv[1])))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
