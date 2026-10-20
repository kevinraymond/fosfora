#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "lightgbm", "scikit-learn", "mir_eval", "scipy"]
# ///
"""Train the beat activation: gradient-boosted trees on the raw per-hop evidence.

    train.py --work <work> --name wide                      # every raw column x offsets -6..0
    train.py --work <work> --name narrow --select-from <work>/models/wide.json --top-cols 16 \
             --leaves 15 --max-depth 5 --trees 200 --export

Training rows come from the labels' `train` split (batida seeds, FMA confident passages,
beatless training clips); the peak picker's (K, thr) and nothing else is chosen on the
`pick` split; the bench TUNE halves and the 155 guard clips are evaluation only, and the
bench HOLDOUT halves are never read here.

Inputs per hop (the runtime has every one of them before the beat detector runs):
  kf.<band>   the kick model's 24 frame features      sf.<part>  SuperFlux combined/low/mid/high
  kp          kick model probability (ungated)         sil        the perceptual-silence flag
  pn.<slot>   the 67 acoustic pre-normalization AudioFeatures slots
each at hop offsets --offsets (default -6..0: causal). Label: y_beat >= 0.5 (the hop LAG
after the beat and its neighbors), weight 1 for positives and 1 - y for the rest, so the
ring around a beat counts less as a negative.

Reported on every evaluation set: per-hop AUC (pooled and per-track mean) for the model
and for `sf.combined` as the onset-function baseline, beat F (mir_eval, 70 ms) after the
causal peak picker for the model, the onset baseline and Fosfora's own grid, mean
activation and picker fires per minute (the beatless sets: lower is better).

--export writes <work>/models/<name>.json in the flat-tree format audio/kick_model.rs
evaluates (format 2: with the input map, lag and picker), after checking the flattened
trees reproduce the booster's raw score.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

import lightgbm as lgb
import mir_eval
import numpy as np
from sklearn.metrics import roc_auc_score

sys.path.insert(0, str(Path(__file__).resolve().parent))
import corpus  # noqa: E402
import hopdump  # noqa: E402

SEED = 0
GROUPS = ("kf", "sf", "kp", "pn", "sil")
MAX_ROWS = dict(batida=1_200_000, fma=2_400_000, negatives_train=800_000)


# ---------------------------------------------------------------- inputs
def base_columns(z, groups):
    """(names, matrix [H, D]) of the raw per-hop columns in `groups`."""
    names, parts = [], []
    if "kf" in groups:
        names += [f"kf.{n}" for n in z["kf_names"]]
        parts.append(z["kf"])
    if "sf" in groups:
        names += [f"sf.{n}" for n in z["sf_names"]]
        parts.append(z["sf"])
    if "kp" in groups:
        names.append("kp")
        parts.append(z["kick_prob"][:, None])
    if "pn" in groups:
        pn_names = list(z["pn_names"])
        keep = [i for i, n in enumerate(pn_names) if n not in hopdump.NOT_ACOUSTIC]
        names += [f"pn.{pn_names[i]}" for i in keep]
        parts.append(z["pn"][:, keep])
    if "sil" in groups:
        names.append("sil")
        parts.append(z["silent"][:, None].astype(np.float32))
    return names, np.hstack(parts).astype(np.float32)


def shifted(X, d):
    """X seen from `d` hops later: row h holds X[h + d] (d <= 0 looks back); zeros outside."""
    S = np.zeros_like(X)
    if d < 0:
        S[-d:] = X[:d]
    elif d > 0:
        S[:-d] = X[d:]
    else:
        S[:] = X
    return S


class Inputs:
    """The model's input layout: (base column, offset) pairs in a fixed order."""

    def __init__(self, pairs):
        self.pairs = list(pairs)  # [(name, offset)]
        self.names = [f"{n}@{d:+d}" for n, d in self.pairs]

    @classmethod
    def full(cls, base_names, offsets):
        return cls([(n, d) for d in offsets for n in base_names])

    @classmethod
    def from_names(cls, names):
        pairs = []
        for s in names:
            n, d = s.rsplit("@", 1)
            pairs.append((n, int(d)))
        return cls(pairs)

    def design(self, base_names, X):
        idx = {n: i for i, n in enumerate(base_names)}
        cols = np.empty((X.shape[0], len(self.pairs)), np.float32)
        cache = {}
        for j, (n, d) in enumerate(self.pairs):
            if d not in cache:
                cache[d] = shifted(X, d)
            cols[:, j] = cache[d][:, idx[n]]
        return cols


def load(work, source, tid):
    z = np.load(corpus.raw_path(work, source, tid))
    lab = np.load(corpus.labels_path(work, source, tid))
    return z, lab


# ---------------------------------------------------------------- rows
def collect(work, tracks, groups, inputs, cap, rng):
    """Design rows from the labeled hops of `tracks`, subsampled per track to about `cap`."""
    per_track = max(1, cap // max(len(tracks), 1))
    Xs, ys, ws = [], [], []
    for source, tid in tracks:
        z, lab = load(work, source, tid)
        w = lab["weight"]
        keep = np.flatnonzero(w > 0)
        if len(keep) == 0:
            continue
        if len(keep) > per_track:
            keep = np.sort(rng.choice(keep, per_track, replace=False))
        names, X = base_columns(z, groups)
        y = lab["y_beat"][keep]
        Xs.append(inputs.design(names, X)[keep])
        ys.append((y >= 0.5).astype(np.int8))
        ws.append(np.where(y >= 0.5, 1.0, np.maximum(1.0 - y, 0.0)).astype(np.float32))
    return np.vstack(Xs), np.concatenate(ys), np.concatenate(ws)


# ---------------------------------------------------------------- picker + scoring
def causal_peaks(a, K, thr, hop_hz):
    """Fire at hop h when a[h] >= thr, a[h] >= every a[h-K+1..h-1] and no fire in the last K."""
    out, last = [], -10 ** 9
    for h in range(len(a)):
        if a[h] < thr or h - last <= K:
            continue
        lo = max(0, h - K + 1)
        if a[h] >= a[lo:h].max(initial=-np.inf):
            out.append(h / hop_hz)
            last = h
    return np.array(out)


def beat_f(ref, est):
    ref = mir_eval.beat.trim_beats(np.asarray(ref, float))
    est = mir_eval.beat.trim_beats(np.asarray(est, float))
    if len(ref) == 0:
        return float("nan")
    if len(est) == 0:
        return 0.0
    return float(mir_eval.beat.f_measure(ref, est, f_measure_threshold=0.07))


def inside(times, passages):
    if len(passages) == 0 or len(times) == 0:
        return np.asarray(times, float)
    t = np.asarray(times, float)
    ok = np.any((t[:, None] >= passages[None, :, 0]) & (t[:, None] <= passages[None, :, 1]), 1)
    return t[ok]


def pick_params(pairs, hop_hz, lag, Ks=(3, 4, 5, 6, 8), thrs=np.arange(0.1, 0.95, 0.05)):
    """(K, thr) maximizing mean beat F over (activation, ref beats, passages) triples; the
    activation is shifted back by `lag` hops before scoring, as the runtime will."""
    best = (-1.0, None, None)
    for K in Ks:
        for thr in thrs:
            fs = []
            for a, ref, pas in pairs:
                est = causal_peaks(a, K, float(thr), hop_hz) - lag / hop_hz
                fs.append(beat_f(ref, inside(est, pas)))
            fs = [f for f in fs if not np.isnan(f)]
            m = float(np.mean(fs)) if fs else 0.0
            if m > best[0]:
                best = (m, K, float(round(thr, 2)))
    return best


def evaluate(work, tracks, predict, groups, inputs, hop_hz, lag, picker, onset_picker, threads):
    rows, pos_pool, neg_pool, on_pos, on_neg = [], [], [], [], []
    for source, tid in tracks:
        z, lab = load(work, source, tid)
        names, X = base_columns(z, groups)
        a = predict(inputs.design(names, X)).astype(np.float32)
        on = z["sf"][:, 0]
        w = lab["weight"] > 0
        y = lab["y_beat"] >= 0.5
        pas = lab["passages"]
        row = dict(id=tid, hops=int(len(a)), mean_act=float(a.mean()), frac_above=float((a >= picker[1]).mean()),
                   fires_per_min=float(len(causal_peaks(a, *picker, hop_hz)) / (len(a) / hop_hz / 60)))
        if (y & w).any() and (~y & w).any():
            row["auc"] = float(roc_auc_score(y[w], a[w]))
            row["auc_onset"] = float(roc_auc_score(y[w], on[w]))
        pos_pool.append(a[y & w]); neg_pool.append(a[~y & w]); on_pos.append(on[y & w]); on_neg.append(on[~y & w])
        ref = lab["label_beats"]
        if len(ref):
            est = causal_peaks(a, *picker, hop_hz) - lag / hop_hz
            est_on = causal_peaks(on, *onset_picker, hop_hz) - 2 / hop_hz  # sf peaks 2 hops after the beat
            row["f_model"] = beat_f(ref, inside(est, pas))
            row["f_onset"] = beat_f(ref, inside(est_on, pas))
            row["f_fosfora"] = beat_f(ref, inside(z["beats"], pas))
        rows.append(row)
    pos, neg = np.concatenate(pos_pool), np.concatenate(neg_pool)
    summ = dict(n_tracks=len(rows))
    if len(pos) and len(neg):
        lbl = np.r_[np.ones(len(pos)), np.zeros(len(neg))]
        summ["auc_pooled"] = float(roc_auc_score(lbl, np.r_[pos, neg]))
        summ["auc_onset_pooled"] = float(roc_auc_score(lbl, np.r_[np.concatenate(on_pos), np.concatenate(on_neg)]))
    for k in ("auc", "auc_onset", "f_model", "f_onset", "f_fosfora", "mean_act", "frac_above", "fires_per_min"):
        v = [r[k] for r in rows if k in r and not np.isnan(r[k])]
        if v:
            summ[f"{k}_mean"] = float(np.mean(v))
    return summ, rows, pos, neg


# ---------------------------------------------------------------- export
def flatten(booster):
    """LightGBM trees as the flat arrays kick_model.rs walks: left when x[feature] <= threshold."""
    dump = booster.dump_model()
    roots, feature, threshold, left, right, value, leaf, depths = [], [], [], [], [], [], [], []

    def walk(node, depth):
        i = len(feature)
        if "leaf_value" in node:
            feature.append(0); threshold.append(0.0); left.append(0); right.append(0)
            value.append(float(node["leaf_value"])); leaf.append(1)
            depths.append(depth)
            return i
        assert node["decision_type"] == "<=", node["decision_type"]
        feature.append(int(node["split_feature"])); threshold.append(float(node["threshold"]))
        left.append(0); right.append(0); value.append(0.0); leaf.append(0)
        left[i] = walk(node["left_child"], depth + 1)
        right[i] = walk(node["right_child"], depth + 1)
        return i

    tree_depths = []
    for t in dump["tree_info"]:
        before = len(depths)
        roots.append(walk(t["tree_structure"], 0))
        tree_depths.append(max(depths[before:]))
    return dict(roots=roots, feature=feature, threshold=threshold, left=left, right=right, value=value, leaf=leaf), tree_depths


def flat_raw_score(flat, X, baseline):
    out = np.full(X.shape[0], baseline, np.float64)
    f, th, L, R, v, lf = (np.asarray(flat[k]) for k in ("feature", "threshold", "left", "right", "value", "leaf"))
    for root in flat["roots"]:
        i = np.full(X.shape[0], root)
        while True:
            internal = lf[i] == 0
            if not internal.any():
                break
            go_left = X[np.arange(X.shape[0]), f[i]] <= th[i]
            i = np.where(internal, np.where(go_left, L[i], R[i]), i)
        out += v[i]
    return out


# ---------------------------------------------------------------- main
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("--name", required=True)
    ap.add_argument("--groups", default=",".join(GROUPS))
    ap.add_argument("--offsets", default="-6:0", help="inclusive hop offset range lo:hi")
    ap.add_argument("--select-from", type=Path, help="a previous run's json: keep its top base columns")
    ap.add_argument("--top-cols", type=int, default=16)
    ap.add_argument("--inputs", type=Path, help="json list of input names (col@offset) to use verbatim")
    ap.add_argument("--trees", type=int, default=300)
    ap.add_argument("--leaves", type=int, default=31)
    ap.add_argument("--max-depth", type=int, default=-1)
    ap.add_argument("--lr", type=float, default=0.08)
    ap.add_argument("--min-leaf", type=int, default=200)
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--sources", default="batida,fma,negatives_train")
    ap.add_argument("--export", action="store_true")
    a = ap.parse_args()
    t0 = time.time()
    rng = np.random.default_rng(SEED)
    groups = tuple(a.groups.split(","))
    lo, hi = (int(v) for v in a.offsets.split(":"))
    offsets = list(range(lo, hi + 1))
    hop_hz = corpus.HOP_HZ
    manifest = json.loads((a.work / "labels" / "manifest.json").read_text())
    lag = int(manifest["provenance"]["target"]["lag_hops"])
    models = a.work / "models"
    models.mkdir(exist_ok=True)

    # the tracks, by split
    labeled = [(s, i) for s, i in corpus.dumped(a.work) if corpus.labels_path(a.work, s, i).exists()]
    train_src = set(a.sources.split(","))
    train = [(s, i) for s, i in labeled if s in train_src and corpus.split_of(s, i) == "train"]
    pick = [(s, i) for s, i in labeled if s in train_src and corpus.split_of(s, i) == "pick"]
    evals = {
        "batida_pick": [(s, i) for s, i in pick if s == "batida"],
        "fma_pick": [(s, i) for s, i in pick if s == "fma"],
        "negatives_train_pick": [(s, i) for s, i in pick if s == "negatives_train"],
        "harmonix_tune": [(s, i) for s, i in labeled if s == "harmonix"],
        "ballroom_tune": [(s, i) for s, i in labeled if s == "ballroom"],
        "negatives_guard": [(s, i) for s, i in labeled if s == "negatives_guard"],
    }

    # the input layout
    z0, _ = load(a.work, *train[0])
    base_names, _ = base_columns(z0, groups)
    if a.inputs:
        inputs = Inputs.from_names(json.loads(a.inputs.read_text()))
    elif a.select_from:
        prev = json.loads(a.select_from.read_text())
        top = [n for n, _ in prev["importance"]["by_column"][: a.top_cols]]
        missing = [n for n in top if n not in base_names]
        assert not missing, f"selected columns not in the current groups: {missing}"
        inputs = Inputs.full(top, offsets)
    else:
        inputs = Inputs.full(base_names, offsets)
    print(f"[{a.name}] {len(train)} train / {len(pick)} pick tracks; {len(inputs.pairs)} inputs "
          f"({len({n for n, _ in inputs.pairs})} columns x offsets {offsets[0]}..{offsets[-1]}); lag {lag}", flush=True)

    # rows
    parts = []
    for src in sorted(train_src):
        tr = [(s, i) for s, i in train if s == src]
        if tr:
            parts.append(collect(a.work, tr, groups, inputs, MAX_ROWS.get(src, 1_000_000), rng))
            print(f"[{a.name}]   {src}: {len(parts[-1][1])} rows, {int(parts[-1][1].sum())} positive", flush=True)
    X = np.vstack([p[0] for p in parts]); y = np.concatenate([p[1] for p in parts]); w = np.concatenate([p[2] for p in parts])
    del parts
    assert np.isfinite(X).all(), "non-finite input"
    print(f"[{a.name}] train rows {len(y)} ({int(y.sum())} positive, {y.mean():.3f}) x {X.shape[1]}, "
          f"loaded in {time.time() - t0:.0f} s", flush=True)

    # fit
    params = dict(objective="binary", learning_rate=a.lr, num_leaves=a.leaves, max_depth=a.max_depth,
                  min_data_in_leaf=a.min_leaf, feature_fraction=0.5, bagging_fraction=0.7, bagging_freq=1,
                  lambda_l2=1.0, scale_pos_weight=float((len(y) - y.sum()) / max(y.sum(), 1)) ** 0.5,
                  num_threads=a.threads, verbose=-1, seed=SEED, deterministic=True)
    ds = lgb.Dataset(X, y, weight=w, feature_name=inputs.names, free_raw_data=False)
    booster = lgb.train(params, ds, num_boost_round=a.trees)
    print(f"[{a.name}] trained {a.trees} trees in {time.time() - t0:.0f} s", flush=True)
    booster.save_model(str(models / f"{a.name}.model.txt"))

    def predict(D):
        return booster.predict(D, num_threads=a.threads)

    # the export check needs the design rows: do it now, before X goes
    flat, tree_depths = flatten(booster)
    idx = rng.choice(len(y), min(4000, len(y)), replace=False)
    raw_ref = booster.predict(X[idx], raw_score=True, num_threads=a.threads)
    raw_flat = flat_raw_score(flat, X[idx].astype(np.float64), 0.0)
    baseline = float(np.median(raw_ref - raw_flat))
    err = float(np.abs(raw_ref - (raw_flat + baseline)).max())
    print(f"[{a.name}] flat trees vs booster: baseline {baseline:+.6f}, max |err| {err:.2e}", flush=True)
    assert err < 1e-4, "flattened trees do not reproduce the booster"
    del X, ds

    # picker on the pick split (batida drummed seeds + FMA passages)
    pairs = []
    for s, i in pick:
        if s == "negatives_train" or (s == "batida" and i.startswith("ambient")):
            continue
        z, lab = load(a.work, s, i)
        names, Xb = base_columns(z, groups)
        pairs.append((predict(inputs.design(names, Xb)), lab["label_beats"], lab["passages"]))
    f_pick, K, thr = pick_params(pairs, hop_hz, lag)
    on_pairs = []
    for s, i in pick:
        if s == "negatives_train" or (s == "batida" and i.startswith("ambient")):
            continue
        z, lab = load(a.work, s, i)
        on_pairs.append((z["sf"][:, 0], lab["label_beats"], lab["passages"]))
    f_on, K_on, thr_on = pick_params(on_pairs, hop_hz, 2)
    picker, onset_picker = (K, thr), (K_on, thr_on)
    print(f"[{a.name}] picker on pick split: model K={K} thr={thr} F={f_pick:.3f}; sf.combined K={K_on} thr={thr_on} F={f_on:.3f}",
          flush=True)

    # evaluation
    out = dict(name=a.name, offsets=offsets, groups=groups, inputs=inputs.names, lag_hops=lag,
               n_train_tracks=len(train), n_train_rows=int(len(y)), n_train_pos=int(y.sum()), params=params,
               trees=a.trees, leaves=a.leaves, max_depth=a.max_depth,
               cost=dict(n_trees=len(flat["roots"]), n_nodes=len(flat["feature"]), sum_depth=int(sum(tree_depths)),
                         max_depth=int(max(tree_depths)), kick_model=dict(n_trees=300, n_nodes=18300, sum_depth=1800)),
               picker=dict(K=K, thr=thr, f_pick=f_pick), onset_picker=dict(K=K_on, thr=thr_on, f_pick=f_on), sets={})
    pools = {}
    for name, tracks in evals.items():
        if not tracks:
            continue
        summ, rows, pos, neg = evaluate(a.work, tracks, predict, groups, inputs, hop_hz, lag, picker, onset_picker, a.threads)
        pools[name] = (pos, neg)
        out["sets"][name] = dict(summary=summ, tracks=rows)
        print(f"[{a.name}] {name}: " + ", ".join(f"{k}={v:.4f}" for k, v in summ.items() if isinstance(v, float)), flush=True)
    for neg_name in ("negatives_guard", "negatives_train_pick"):
        if neg_name in pools and "harmonix_tune" in pools:
            hp, nn = pools["harmonix_tune"][0], pools[neg_name][1]
            v = float(roc_auc_score(np.r_[np.ones(len(hp)), np.zeros(len(nn))], np.r_[hp, nn]))
            out["sets"][neg_name]["summary"]["auc_vs_harmonix_positives"] = v
            print(f"[{a.name}] {neg_name} vs harmonix positives AUC {v:.4f}", flush=True)

    # importance
    gain = booster.feature_importance("gain")
    by_input = sorted(zip(inputs.names, (float(g) for g in gain)), key=lambda t: -t[1])
    by_col, by_off = {}, {}
    for (n, d), g in zip(inputs.pairs, gain):
        by_col[n] = by_col.get(n, 0.0) + float(g)
        by_off[d] = by_off.get(d, 0.0) + float(g)
    out["importance"] = dict(by_input=by_input[:60], by_column=sorted(by_col.items(), key=lambda t: -t[1]),
                             by_offset=sorted(by_off.items()))
    print(f"[{a.name}] top columns: " + ", ".join(f"{n} {g / sum(by_col.values()):.2f}" for n, g in out["importance"]["by_column"][:12]))
    print(f"[{a.name}] by offset: " + ", ".join(f"{d:+d}:{g / sum(by_off.values()):.2f}" for d, g in out["importance"]["by_offset"]))

    try:
        commit = subprocess.run(["git", "-C", str(corpus.REPO), "rev-parse", "--short", "HEAD"],
                                capture_output=True, text=True, check=True).stdout.strip()
    except Exception:  # noqa: BLE001
        commit = None
    out["provenance"] = dict(repo_commit=commit, labels_manifest=manifest["provenance"],
                             counts=manifest.get("counts"), seconds=round(time.time() - t0))
    (models / f"{a.name}.json").write_text(json.dumps(out, indent=1))
    if a.export:
        model = dict(format=2, kind="beat_activation", lag_hops=lag,
                     inputs=[dict(name=n, offset=d) for n, d in inputs.pairs], n_features=len(inputs.pairs),
                     baseline=baseline, picker=dict(k=K, thr=thr), **flat,
                     trained=dict(repo_commit=commit, name=a.name, trees=a.trees, leaves=a.leaves, max_depth=a.max_depth,
                                  n_train_rows=int(len(y)), labels_manifest_target=manifest["provenance"]["target"]))
        (models / f"{a.name}.beat_model.json").write_text(json.dumps(model, separators=(",", ":")))
        print(f"[{a.name}] exported {models / (a.name + '.beat_model.json')} "
              f"({len(flat['feature'])} nodes, {len(inputs.pairs)} inputs)")
    print(f"[{a.name}] done in {time.time() - t0:.0f} s -> {models / (a.name + '.json')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
