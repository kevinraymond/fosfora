#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Targets, weights and splits for every dumped track: <work>/labels/<source>/<id>.npz.

    build_dataset.py --work <work> [--lag 3] [--sigma 1.0] [--only fma,batida] [--lag-check]

The raw evidence stays in <work>/raw (dump_track.py); this adds, per track:
  y_beat      float32 [H]  Gaussian target (sigma hops) centered LAG hops after each label
                           beat: the hop whose frame has taken the onset in (see --lag-check)
  y_downbeat  float32 [H]  the same for downbeats
  weight      float32 [H]  1 = labeled hop, 0 = unlabeled (FMA outside confident passages)
  label_beats, label_downbeats  float64  label times (s)
  passages    float64 [P, 2]  the labeled spans (s); the whole track where labels are exact
  split       str  train | pick | eval  (corpus.split_of)
  meta        json

Label rules:
  batida           exact beats / downbeats from the render json; the ambient template has
                   no drums and is all-negative (y = 0, weight 1)
  fma              Fosfora's own beats, kept only inside CONFIDENT passages: tracker locked
                   with support >= MIN_SUPPORT at the latest tempo update, inside an episode
                   with no lock / unlock / arbiter shift / anchor event / seed and no
                   published-tempo change > TEMPO_CHANGE, in a run of >= MIN_RUN such beats;
                   everything else weight 0
                   With --fma-oracle DIR (#4288): an offline tracker's beats and downbeats
                   instead (DIR/<id>.json, Beat This), over the whole track: weight 1 from half
                   a beat before its first beat to half a beat after its last, cut where it
                   skips (an interval over ORACLE_GAP x the local median); a track with fewer
                   than ORACLE_MIN_BEATS beats or under ORACLE_STABLE of its intervals within
                   +-ORACLE_TOL of the local median is left unlabeled (weight 0)
  negatives_*      all hops negative, weight 1
  harmonix, ballroom  bench reference beats (TUNE halves; eval only)

--lag-check prints, over the drummed batida renders, the offset at which each raw column
peaks relative to the exact beat labels, which is how LAG was chosen.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import corpus  # noqa: E402

MIN_SUPPORT = 0.8
MIN_RUN = 16
TEMPO_CHANGE = 0.02
SPLIT_EVENTS = {"lock", "unlock", "arb_shift", "anchor_displaced", "anchor_abandoned",
                "stale_reanchor", "seed", "first_anchor"}
MODE_LOCKED = 3  # dump_track.MODES.index("locked")
ORACLE_MIN_BEATS = 32
ORACLE_GAP = 1.8
ORACLE_TOL = 0.15
ORACLE_STABLE = 0.8
ORACLE_MED_WIN = 9


def label_hop(times, hop_hz):
    """The hop whose analysis frame first contains time t: frame h ends at sample (h+1)*hop."""
    return np.floor(np.asarray(times, np.float64) * hop_hz).astype(int)


def gaussian_target(times, n_hops, hop_hz, sigma, lag):
    y = np.zeros(n_hops, np.float32)
    if len(times) == 0:
        return y
    centers = label_hop(times, hop_hz) + lag
    r = int(np.ceil(4 * sigma))
    for c in centers:
        lo, hi = max(0, c - r), min(n_hops, c + r + 1)
        if lo >= hi:
            continue
        k = np.arange(lo, hi)
        y[lo:hi] = np.maximum(y[lo:hi], np.exp(-0.5 * ((k - c) / sigma) ** 2))
    return y


def confident_passages(z):
    """FMA pseudo-label selection. Returns (passages [[t0, t1]...], kept beat times, stats)."""
    beats = z["beats"]
    ts, sup, locked, pub, mode = z["sc_ts"], z["sc_support"], z["sc_locked"], z["sc_pub_bpm"], z["sc_mode"]
    stats = dict(n_fired=int(len(beats)), n_updates=int(len(ts)))
    if len(beats) == 0 or len(ts) == 0:
        return np.zeros((0, 2)), np.zeros(0), stats
    cuts = set(float(t) for t, n in zip(z["ev_ts"], z["ev_name"]) if n in SPLIT_EVENTS)
    pub_prev = pub[0]
    for t, p in zip(ts[1:], pub[1:]):
        if pub_prev > 0 and p > 0 and abs(p - pub_prev) / pub_prev > TEMPO_CHANGE:
            cuts.add(float(t))
        pub_prev = p
    cuts = np.array(sorted(cuts))
    j = np.searchsorted(ts, beats, side="right") - 1
    ok = j >= 0
    jj = np.maximum(j, 0)
    ok &= (locked[jj] > 0.5) & (sup[jj] >= MIN_SUPPORT) & (mode[jj] == MODE_LOCKED)
    episode = np.searchsorted(cuts, beats, side="right")
    stats["n_beats_locked_supported"] = int(ok.sum())
    passages, kept = [], []
    i, n = 0, len(beats)
    while i < n:
        if not ok[i]:
            i += 1
            continue
        k = i
        while k + 1 < n and ok[k + 1] and episode[k + 1] == episode[i]:
            k += 1
        if k - i + 1 >= MIN_RUN:
            period = (beats[k] - beats[i]) / (k - i)
            t0, t1 = beats[i] - 0.5 * period, beats[k] + 0.5 * period
            lo = cuts[episode[i] - 1] if episode[i] > 0 else 0.0
            hi = cuts[episode[i]] if episode[i] < len(cuts) else np.inf
            passages.append([max(t0, lo), min(t1, hi)])
            kept.extend(beats[i:k + 1].tolist())
        i = k + 1
    stats["n_runs"] = len(passages)
    stats["n_kept"] = len(kept)
    return np.array(passages).reshape(-1, 2), np.array(kept), stats


def oracle_passages(beats):
    """#4288: labeled spans for an offline tracker's beats, and why a track is left out."""
    beats = np.asarray(beats, np.float64)
    if len(beats) < ORACLE_MIN_BEATS:
        return np.zeros((0, 2)), f"{len(beats)} beats < {ORACLE_MIN_BEATS}"
    ibi = np.diff(beats)
    h = ORACLE_MED_WIN // 2
    med = np.array([np.median(ibi[max(0, i - h):i + h + 1]) for i in range(len(ibi))])
    stable = float(np.mean(np.abs(ibi / med - 1.0) <= ORACLE_TOL))
    if stable < ORACLE_STABLE:
        return np.zeros((0, 2)), f"stable intervals {stable:.2f} < {ORACLE_STABLE}"
    passages, start = [], 0
    for i, (x, m) in enumerate(zip(ibi, med)):
        if x > ORACLE_GAP * m:
            if i > start:
                passages.append([beats[start] - 0.5 * med[start], beats[i] + 0.5 * m])
            start = i + 1
    if start < len(beats) - 1:
        passages.append([beats[start] - 0.5 * med[min(start, len(med) - 1)], beats[-1] + 0.5 * med[-1]])
    return np.array(passages).reshape(-1, 2), None


def write_track(out, z, label_beats, label_downbeats, weight, passages, split, lag, sigma, extra):
    n = z["ts"].shape[0]
    raw_meta = json.loads(str(z["meta"]))
    hop_hz = float(raw_meta["hop_hz"])
    y = gaussian_target(label_beats, n, hop_hz, sigma, lag)
    yd = gaussian_target(label_downbeats, n, hop_hz, sigma, lag)
    meta = dict(
        source=extra.pop("source"), split=split, lag_hops=lag, sigma_hops=sigma, hop_hz=hop_hz, n_hops=n,
        duration_s=raw_meta["duration_s"], audio=raw_meta["audio"], audio_sha256=raw_meta["audio_sha256"],
        n_label_beats=int(len(label_beats)), labeled_hops=int(weight.sum()),
        positive_hops=int(((y >= 0.5) & (weight > 0)).sum()), **extra,
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".part.npz")
    np.savez_compressed(
        tmp, y_beat=y, y_downbeat=yd, weight=weight.astype(np.float32),
        label_beats=np.asarray(label_beats, np.float64), label_downbeats=np.asarray(label_downbeats, np.float64),
        passages=np.asarray(passages, np.float64).reshape(-1, 2), split=np.array(split),
        meta=np.array(json.dumps(meta)))
    tmp.replace(out)
    return meta


def build(work: Path, lag: int, sigma: float, only, fma_oracle=None):
    rows = Counter()
    labeled_min = Counter()
    per_source = {}
    fma = corpus.fma_rows(work)
    neg = corpus.negatives_train_rows(work)
    for source, tid in corpus.dumped(work):
        if only is not None and source not in only:
            continue
        z = np.load(corpus.raw_path(work, source, tid))
        n = z["ts"].shape[0]
        hop_hz = float(json.loads(str(z["meta"]))["hop_hz"])
        whole = [[0.0, n / hop_hz]]
        split = corpus.split_of(source, tid)
        out = corpus.labels_path(work, source, tid)
        if source == "batida":
            lab = corpus.batida_labels(work, tid)
            template, seed = tid.rsplit("-", 1)
            beats, downs = ([], []) if template == "ambient" else (lab["beats"], lab["downbeats"])
            m = write_track(out, z, beats, downs, np.ones(n), whole, split, lag, sigma,
                            dict(source=source, template=template, seed=int(seed), tempo_bpm=lab["tempo_bpm"],
                                 label_rule="exact render labels; ambient all-negative"))
        elif source == "fma" and fma_oracle is not None:
            r = fma.get(tid, {})
            o = json.loads((fma_oracle / f"{tid}.json").read_text())
            passages, why = oracle_passages(o["beats"])
            passages[:, 0] = np.maximum(passages[:, 0], 0.0)
            weight = np.zeros(n)
            for t0, t1 in passages:
                weight[int(np.floor(t0 * hop_hz)):int(np.ceil(t1 * hop_hz)) + 1] = 1.0
            beats = np.asarray(o["beats"]) if why is None else np.zeros(0)
            downs = np.asarray(o["downbeats"]) if why is None else np.zeros(0)
            m = write_track(out, z, beats, downs, weight, passages, split, lag, sigma,
                            dict(source=source, fma_id=int(tid), license=r.get("lic"), artist=r.get("artist"),
                                 genre=r.get("sgenre") or "untagged", oracle_excluded=why,
                                 label_rule="Beat This (final0, no DBN) beats + downbeats over the track, "
                                            f"cut at gaps > {ORACLE_GAP}x the local median interval"))
        elif source == "fma":
            r = fma.get(tid, {})
            passages, kept, stats = confident_passages(z)
            weight = np.zeros(n)
            for t0, t1 in passages:
                weight[int(np.floor(t0 * hop_hz)):int(np.ceil(t1 * hop_hz)) + 1] = 1.0
            down = z["downbeats"]
            if len(passages):
                inside = np.any((down[:, None] >= passages[None, :, 0]) & (down[:, None] <= passages[None, :, 1]), 1)
                down = down[inside]
            else:
                down = np.zeros(0)
            m = write_track(out, z, kept, down, weight, passages, split, lag, sigma,
                            dict(source=source, fma_id=int(tid), license=r.get("lic"), artist=r.get("artist"),
                                 genre=r.get("sgenre") or "untagged",
                                 label_rule=f"fosfora beats, locked & support>={MIN_SUPPORT}, runs>={MIN_RUN}, "
                                            f"no split event / tempo change>{TEMPO_CHANGE}",
                                 **{f"sel_{k}": v for k, v in stats.items()}))
        elif source in ("negatives_train", "negatives_guard"):
            r = neg.get(tid, {}) if source == "negatives_train" else {}
            m = write_track(out, z, [], [], np.ones(n), whole, split, lag, sigma,
                            dict(source=source, genre=r.get("genre"), license=r.get("license"), pulse=r.get("pulse"),
                                 n_fired=int(len(z["beats"])), label_rule="beatless clip: every hop negative"))
        elif source in ("harmonix", "ballroom"):
            a = corpus.bench_annotation(source, tid)
            m = write_track(out, z, a["beats"], a.get("downbeats") or [], np.ones(n), whole, split, lag, sigma,
                            dict(source=source, label_rule="bench reference beats (TUNE half)"))
        else:
            print(f"unknown source {source}, skipped", file=sys.stderr)
            continue
        rows[(source, split)] += 1
        labeled_min[(source, split)] += m["labeled_hops"] / hop_hz / 60
        per_source.setdefault(source, []).append(dict(id=tid, split=split, hops=n, labeled_hops=m["labeled_hops"],
                                                      positive_hops=m["positive_hops"], n_label_beats=m["n_label_beats"]))
    return rows, labeled_min, per_source


def lag_check(work: Path, radius=6):
    """Offset (hops) at which each raw column peaks around the exact batida beat labels."""
    acc = {}
    n_beats = 0
    for source, tid in corpus.dumped(work, ("batida",)):
        if tid.startswith("ambient"):
            continue
        z = np.load(corpus.raw_path(work, source, tid))
        lab = corpus.batida_labels(work, tid)
        hop_hz = float(json.loads(str(z["meta"]))["hop_hz"])
        pn = list(z["pn_names"])
        cols = {
            "kf_flux_sum": z["kf"][:, :11].sum(1), "kf_flux_low5": z["kf"][:, :5].sum(1),
            "sf_combined": z["sf"][:, 0], "kick_prob": z["kick_prob"],
            "pn_flux": z["pn"][:, pn.index("flux")], "pn_rms": z["pn"][:, pn.index("rms")],
            "pn_timbre_flux": z["pn"][:, pn.index("timbre_flux")],
        }
        hops = label_hop(lab["beats"], hop_hz)
        hops = hops[(hops >= radius) & (hops + radius < len(z["ts"]))]
        n_beats += len(hops)
        win = hops[:, None] + np.arange(-radius, radius + 1)[None, :]
        for name, c in cols.items():
            offs = np.argmax(c[win], axis=1) - radius
            acc.setdefault(name, np.zeros(2 * radius + 1, int))
            acc[name] += np.bincount(offs + radius, minlength=2 * radius + 1)
    print(f"peak offset relative to {n_beats} exact beat labels (hops; frame h ends at sample (h+1)*512)")
    for name, hist in acc.items():
        mode = int(np.argmax(hist)) - radius
        mean = float((hist * np.arange(-radius, radius + 1)).sum() / max(hist.sum(), 1))
        print(f"  {name:15s} mode {mode:+d}  mean {mean:+.2f}  share at mode {hist.max() / max(hist.sum(), 1):.2f}")
    return {name: dict(mode=int(np.argmax(h)) - radius, hist=h.tolist()) for name, h in acc.items()}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("--lag", type=int, default=3)
    ap.add_argument("--sigma", type=float, default=1.0)
    ap.add_argument("--only", help="comma-separated sources")
    ap.add_argument("--lag-check", action="store_true", help="print the peak-offset table and exit")
    ap.add_argument("--fma-oracle", type=Path, help="label FMA tracks with <dir>/<id>.json beats (#4288)")
    a = ap.parse_args()
    if a.lag_check:
        lag_check(a.work)
        return 0
    only = set(a.only.split(",")) if a.only else None
    rows, labeled_min, per_source = build(a.work, a.lag, a.sigma, only, a.fma_oracle)
    first_raw = next(iter(corpus.dumped(a.work)), None)
    raw_meta = json.loads(str(np.load(corpus.raw_path(a.work, *first_raw))["meta"])) if first_raw else {}
    try:
        commit = subprocess.run(["git", "-C", str(corpus.REPO), "rev-parse", "--short", "HEAD"],
                                capture_output=True, text=True, check=True).stdout.strip()
    except Exception:  # noqa: BLE001
        commit = None
    mpath = a.work / "labels" / "manifest.json"
    manifest = json.loads(mpath.read_text()) if mpath.exists() else {}
    manifest["provenance"] = dict(
        fosfora_binary=raw_meta.get("binary"), fosfora_sha256=raw_meta.get("binary_sha256"), repo_commit=commit,
        dump_cmd="--signal-dump <audio> --out -, FOSFORA_HOP_SIDECAR + FOSFORA_BEAT_SIDECAR (dump_track.py)",
        hop_hz=corpus.HOP_HZ, target=dict(kind="gaussian", sigma_hops=a.sigma, lag_hops=a.lag),
        label_hop="floor(t * hop_hz): the frame that first contains t",
        split=dict(rule="last digit of batida seed / track id in {8, 9} -> pick, else train; bench TUNE halves "
                        "and the guard clips -> eval; bench HOLDOUT never read here", pick_digits=sorted(corpus.PICK_DIGITS)),
        fma_rule=dict(min_support=MIN_SUPPORT, min_run_beats=MIN_RUN, tempo_change_frac=TEMPO_CHANGE,
                      split_events=sorted(SPLIT_EVENTS)) if a.fma_oracle is None else
                 dict(oracle=str(a.fma_oracle), tracker="Beat This final0, no DBN", min_beats=ORACLE_MIN_BEATS,
                      gap=ORACLE_GAP, tol=ORACLE_TOL, stable=ORACLE_STABLE, median_window=ORACLE_MED_WIN),
        lag_check=lag_check(a.work),
    )
    manifest["counts"] = {f"{s}/{sp}": dict(tracks=n, labeled_minutes=round(labeled_min[(s, sp)], 1))
                          for (s, sp), n in sorted(rows.items())}
    for s, lst in per_source.items():
        manifest[s] = lst
    mpath.parent.mkdir(parents=True, exist_ok=True)
    mpath.write_text(json.dumps(manifest, indent=1))
    for (s, sp), n in sorted(rows.items()):
        print(f"{s:16s} {sp:5s} {n:4d} tracks, {labeled_min[(s, sp)]:7.1f} labeled min")
    print(f"wrote {mpath}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
