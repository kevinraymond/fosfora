#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Replay and sweep the drop state machine's arm/fire logic (#2211, workstream Q).

    bench/sweep_drop.py --validate            # gate: replay must reproduce the binary
    bench/sweep_drop.py --stage arm           # then sweep, tune half only
    bench/sweep_drop.py --stage combined --top 12

Input is the v2 structure sidecar (`bench/dump_structure_sidecar.py`), which carries the
drop machine's exact per-tick inputs — the pre-normalization `loudness_m`/`sub_bass` the
detector reads, not the wire proxies a dump exposes. That makes this replay *exact*
rather than approximate: `--validate` requires it to reproduce the shipped binary's fired
ticks bit-for-bit, and a mismatch is a hard failure. A replay that has drifted makes every
sweep number a lie.

Swept: everything `update_drop` reads except the build-up logistic's own weights (the
logistic's inputs are not recorded; `cur_buildup` is, so the arm machine downstream of it
is fully replayable).

Scoring follows benchlib's drop convention: a hit is within +-1 bar of an annotated drop
(bar length measured locally), greedy one-to-one by ascending |dt|; every unmatched
estimate is a false alarm, including on the 155 zero-drop tune tracks whose only
contribution is false alarms.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import sys
from dataclasses import dataclass, replace
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parent.parent
TICK_DT = 0.1
# benchlib convention (metrics/__init__.py): +-1 bar, bar measured locally over +-16 s.
MATCH_BARS = 1.0
LOCAL_S = 16.0
FALLBACK_BAR_S = 2.0


def is_tune(track_id: str) -> bool:
    # Canonical split rule: analyze_predict_drop.py (sha256 parity).
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


# =================================================================================
# The machine under test — a direct port of audio/structure.rs::update_drop
# =================================================================================


@dataclass(frozen=True)
class DropCfg:
    """Shipped defaults mirror `StructureConfig::default()`."""

    arm_buildup: float = 0.6
    arm_sustain: float = 4.0
    loud_jump: float = 0.08
    subbass_return: float = 0.5
    refractory: float = 16.0
    # --- levers this round adds ---
    # Ticks the baseline running-min spans. None = each track's own shipped ring, which
    # is what the binary used. That ring is sized at the analysis frame rate but pushed at
    # the 10 Hz tick, so DROP_BASELINE_SECONDS naming 1.5 s produced 129 ticks (12.9 s) on
    # 44.1 kHz material and 141 (14.1 s) on 48 kHz — the window has never been 1.5 s, and
    # has never been the same window twice across sample rates (finding #2212).
    baseline_ticks: int | None = None
    # Multiplier on the arm timer's decay while build-up sits below the arm level.
    arm_decay: float = 2.0
    # Seconds the arm stays live after the timer falls short. 0.0 = shipped behaviour
    # (disarm immediately), which makes the pre-drop gap disarm the machine right when
    # the drop lands.
    arm_hold: float = 0.0


def simulate(t: np.ndarray, build: np.ndarray, loud: np.ndarray, sub: np.ndarray,
             sub_ref: np.ndarray, cfg: DropCfg, shipped_ticks: int = 141) -> np.ndarray:
    """Return the times of fired drops. Mirrors update_drop tick for tick.

    The arm timer accumulates in **float32**, because the detector does and the rounding
    is load-bearing: at the one true positive in the specimen set the f32 sum is still a
    hair under the 4.0 s requirement on the tick f64 has already crossed it, so an f64
    replay fires a tick early. The margin there is 0.1 s of a 4.0 s threshold — the
    shipped detector arms that close to the edge.
    """
    f32 = np.float32
    high = f32(0.0)
    dt = f32(TICK_DT)
    decay = f32(cfg.arm_decay) * dt
    level = f32(cfg.arm_buildup)
    sustain = f32(cfg.arm_sustain)
    gate = f32(cfg.loud_jump)
    sub_frac = f32(cfg.subbass_return)
    refractory_until = -np.inf
    armed_until = -np.inf
    ring: list[np.float32] = []
    baseline_ticks = cfg.baseline_ticks if cfg.baseline_ticks is not None else shipped_ticks
    out = []
    for i in range(len(t)):
        now = float(t[i])
        if f32(build[i]) > level:
            high = f32(high + dt)
        else:
            high = max(f32(0.0), f32(high - decay))

        ring.append(f32(loud[i]))
        if len(ring) > baseline_ticks:
            ring.pop(0)
        jump = f32(f32(loud[i]) - min(ring))
        sub_returning = f32(sub[i]) > f32(sub_frac * f32(sub_ref[i]))

        if high >= sustain:
            armed_until = now + cfg.arm_hold
        armed = high >= sustain or now <= armed_until

        if armed and now >= refractory_until and jump >= gate and sub_returning:
            refractory_until = now + cfg.refractory
            armed_until = -np.inf
            high = f32(0.0)
            out.append(now)
    return np.array(out, dtype=np.float64)


# =================================================================================
# Sidecars + annotations
# =================================================================================


class CorruptSidecar(Exception):
    pass


def check_integrity(tid: str, records: list[dict]) -> None:
    """Tick indices must run 1, 2, 3, ... — the detector emits one per tick, in order.

    A gap or repeat means the file is not one clean run of one track: two dumpers sharing
    an output directory interleave into the same temp file, and the result still parses as
    JSONL. Content, not syntax, is what catches that.
    """
    prev = 0
    for r in records:
        cur = int(r["d_tick"])
        if cur != prev + 1:
            raise CorruptSidecar(
                f"{tid}: tick index jumped {prev} -> {cur} (interleaved or truncated dump)")
        prev = cur


class Track:
    __slots__ = ("tid", "t", "build", "loud", "sub", "sub_ref", "fired",
                 "refs", "beats", "downbeats", "duration", "shipped_ticks")

    def __init__(self, tid: str, records: list[dict], ann: dict):
        check_integrity(tid, records)
        self.tid = tid
        self.t = np.array([r["d_t"] for r in records], dtype=np.float64)
        self.build = np.array([r["d_build"] for r in records], dtype=np.float64)
        self.loud = np.array([r["d_loud_m"] for r in records], dtype=np.float64)
        self.sub = np.array([r["d_sub"] for r in records], dtype=np.float64)
        self.sub_ref = np.array([r["d_sub_ref"] for r in records], dtype=np.float64)
        self.fired = self.t[np.array([r["d_fired"] for r in records]) > 0.5]
        # The binary's own ring capacity for this track — sample-rate dependent, so read
        # it from the data rather than assuming one number for the corpus.
        self.shipped_ticks = int(max(r["d_ring"] for r in records))
        self.refs = np.array([d["time"] for d in ann.get("drops", [])], dtype=np.float64)
        self.duration = float(ann["audio"]["duration_s"])
        db = ann.get("downbeats") or []
        bt = ann.get("beats") or []
        self.downbeats = np.array(db, dtype=np.float64) if db else None
        self.beats = np.array(bt, dtype=np.float64) if bt else None


def load_specimens(sidecar_dir: Path, refs_path: Path) -> list[Track]:
    """The three AI-mastered specimen tracks (finding #2212), scored like a dataset.

    References come from `drop_reference.py` (raw-audio sub-bass withdrawal->return with an
    RMS step, each corroborated by an independent Q4 boundary announcement). They carry no
    beat annotation, so the +-1 bar tolerance falls back to 2 s.
    """
    refs = json.loads(refs_path.read_text())
    out = []
    for name, ref in sorted(refs.items()):
        p = sidecar_dir / f"{name}.jsonl"
        if not p.exists():
            continue
        records = [r for r in (json.loads(line) for line in p.read_text().splitlines()
                               if line.strip()) if "meta" not in r]
        if not records or "d_fired" not in records[0]:
            sys.exit(f"{p}: schema v1 sidecar — re-dump")
        out.append(Track(name, records,
                         {"drops": ref["drops"], "audio": {"duration_s": ref["duration_s"]}}))
    if not out:
        sys.exit(f"no specimen sidecars in {sidecar_dir}")
    return out


# The subset finding #2209 hand-checked as textbook drops — the pre-registered gate.
# Everything else in the specimen set is mechanically derived and reported descriptively.
HAND_VERIFIED = {("Thermal Break", 47.8), ("Thirty Two Hertz", 75.15),
                 ("Thirty Two Hertz", 150.15)}


def specimen_detail(tracks: list[Track], cfg: DropCfg) -> tuple[int, int, list[str]]:
    """(hand-verified hits, total hits, per-reference lines)."""
    hv = hits = 0
    lines = []
    for tr in tracks:
        est = simulate(tr.t, tr.build, tr.loud, tr.sub, tr.sub_ref, cfg, tr.shipped_ticks)
        for rt in tr.refs:
            tol = MATCH_BARS * bar_seconds(tr, float(rt))
            near = [e for e in est if abs(e - rt) <= tol]
            ok = bool(near)
            gate = any(abs(rt - h) < 0.2 for n, h in HAND_VERIFIED if n == tr.tid)
            hits += ok
            hv += ok and gate
            lines.append(f"    {'HIT ' if ok else 'miss'} {tr.tid:18} @{rt:7.2f}s"
                         f"{'  [hand-verified]' if gate else ''}")
        extra = len(est) - sum(1 for rt in tr.refs
                               if any(abs(e - rt) <= MATCH_BARS * bar_seconds(tr, float(rt))
                                      for e in est))
        if extra > 0:
            lines.append(f"    +{extra} unmatched fire(s) on {tr.tid}")
    return hv, hits, lines


def bar_seconds(tr: Track, at: float) -> float:
    """Local bar length, benchlib's rule: median downbeat interval near `at`."""
    for times, mult in ((tr.downbeats, 1.0), (tr.beats, 4.0)):
        if times is None or len(times) < 2:
            continue
        local = times[(times >= at - LOCAL_S) & (times <= at + LOCAL_S)]
        base = local if len(local) >= 2 else times
        return float(np.median(np.diff(base))) * mult
    return FALLBACK_BAR_S


def load_tracks(dataset: Path, tune_only: bool = True,
                allow_partial: bool = False) -> list[Track]:
    sidecar_dir = REPO / "bench" / "out" / "structsweep" / dataset.name
    if not sidecar_dir.exists():
        sys.exit(f"no sidecars at {sidecar_dir} — run bench/dump_structure_sidecar.py")
    out, v1 = [], 0
    for p in sorted(sidecar_dir.glob("*.jsonl")):
        tid = p.stem
        if tune_only and not is_tune(tid):
            continue
        ann_path = dataset / "norm" / f"{tid}.json"
        if not ann_path.exists():
            continue
        records = []
        for line in p.read_text().splitlines():
            if not line.strip():
                continue
            r = json.loads(line)
            if "meta" in r:
                continue
            if "d_fired" not in r:
                v1 += 1
                records = []
                break
            records.append(r)
        if len(records) < 100:
            continue
        out.append(Track(tid, records, json.loads(ann_path.read_text())))
    if v1:
        # Never silent: a sweep that quietly dropped a third of the corpus would read as
        # having covered it.
        msg = f"{v1} sidecars are schema v1 (no drop trace) — re-dump with --force"
        if not allow_partial:
            sys.exit(msg)
        print(f"WARNING: skipping {msg}; {len(out)} tracks scored")
    if not out:
        sys.exit("no usable sidecars")
    return out


# =================================================================================
# Scoring
# =================================================================================


def score(tracks: list[Track], cfg: DropCfg, cache: dict | None = None) -> dict:
    hits = misses = false = 0
    total_min = 0.0
    per_track = []
    for tr in tracks:
        est = simulate(tr.t, tr.build, tr.loud, tr.sub, tr.sub_ref, cfg, tr.shipped_ticks)
        # Greedy one-to-one by ascending |dt|, +-1 bar measured locally.
        pairs = []
        for ri, rt in enumerate(tr.refs):
            tol = MATCH_BARS * bar_seconds(tr, float(rt))
            for ei, et in enumerate(est):
                if abs(et - rt) <= tol:
                    pairs.append((abs(et - rt), ri, ei))
        pairs.sort()
        used_r, used_e = set(), set()
        for _, ri, ei in pairs:
            if ri in used_r or ei in used_e:
                continue
            used_r.add(ri)
            used_e.add(ei)
        h = len(used_r)
        hits += h
        misses += len(tr.refs) - h
        false += len(est) - len(used_e)
        total_min += tr.duration / 60.0
        per_track.append((tr.tid, h, len(tr.refs), len(est) - len(used_e)))
    n_ref = hits + misses
    return {
        "hits": hits, "misses": misses, "false": false,
        "n_refs": n_ref, "n_tracks": len(tracks),
        "recall": hits / n_ref if n_ref else 0.0,
        "precision": hits / (hits + false) if (hits + false) else 0.0,
        "fa_per_min": false / total_min if total_min else 0.0,
        "minutes": total_min,
        "est_per_track": (hits + false) / len(tracks) if tracks else 0.0,
        "per_track": per_track,
    }


def fmt(cfg: DropCfg, s: dict) -> str:
    return (f"lvl {cfg.arm_buildup:.2f} sus {cfg.arm_sustain:4.1f} hold {cfg.arm_hold:4.1f} "
            f"dec {cfg.arm_decay:.1f} jump {cfg.loud_jump:.3f} "
            f"base {cfg.baseline_ticks if cfg.baseline_ticks is not None else 'shipped':>7} "
            f"sub {cfg.subbass_return:.2f} | "
            f"recall {s['recall']:.3f} ({s['hits']}/{s['n_refs']})  "
            f"prec {s['precision']:.3f}  FA/min {s['fa_per_min']:.3f}  "
            f"est/track {s['est_per_track']:.2f}")


# =================================================================================
# Stages
# =================================================================================


def stage_grid(stage: str) -> list[DropCfg]:
    base = DropCfg()
    if stage == "arm":
        # The arm is the sole blocker at 11/11 specimen misses (#2212): level, the
        # sustain requirement, the dip decay, and holding the arm through the pre-drop
        # gap. Fire side held at shipped.
        return [replace(base, arm_buildup=lv, arm_sustain=su, arm_hold=ho, arm_decay=de)
                for lv, su, ho, de in itertools.product(
                    (0.35, 0.4, 0.45, 0.5, 0.55, 0.6),
                    (2.0, 3.0, 4.0, 6.0),
                    (0.0, 2.0, 4.0, 8.0),
                    (1.0, 2.0))]
    if stage == "fire":
        return [replace(base, loud_jump=j, subbass_return=sb, baseline_ticks=bt)
                for j, sb, bt in itertools.product(
                    (0.04, 0.06, 0.08, 0.12, 0.16),
                    (0.3, 0.4, 0.5, 0.6, 0.7),
                    (15, 30, 60, 129, 200))]
    if stage == "refractory":
        return [replace(base, refractory=r) for r in (8.0, 12.0, 16.0, 24.0, 32.0)]
    sys.exit(f"unknown stage {stage}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--dataset", type=Path, default=REPO / "bench" / "datasets" / "harmonix")
    ap.add_argument("--validate", action="store_true",
                    help="replay the shipped config and require an exact match to the binary")
    ap.add_argument("--stage", choices=("arm", "fire", "refractory", "combined"))
    ap.add_argument("--fa-budget", type=float, default=None,
                    help="only report configs at or under this FA/min")
    ap.add_argument("--top", type=int, default=15)
    ap.add_argument("--holdout", action="store_true",
                    help="score the HOLDOUT half — the once-only final measurement")
    ap.add_argument("--config", type=str, default=None,
                    help="JSON dict of DropCfg overrides to score on its own")
    ap.add_argument("--specimens", action="store_true",
                    help="score the three AI-mastered specimen tracks instead of a dataset")
    ap.add_argument("--allow-partial", action="store_true",
                    help="score only the v2 sidecars present, reporting what was skipped")
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    if args.specimens:
        tracks = load_specimens(REPO / "bench" / "out" / "dropsweep" / "music",
                                REPO / "bench" / "out" / "dropsweep" / "music_refs.json")
        print(f"specimens: {len(tracks)} tracks, "
              f"{sum(len(t.refs) for t in tracks)} references "
              f"({len(HAND_VERIFIED)} hand-verified)")
    elif args.holdout:
        tracks = [t for t in load_tracks(args.dataset, tune_only=False,
                                         allow_partial=args.allow_partial)
                  if not is_tune(t.tid)]
        print(f"HOLDOUT: {len(tracks)} tracks")
    else:
        tracks = load_tracks(args.dataset, tune_only=True,
                             allow_partial=args.allow_partial)
        print(f"tune half: {len(tracks)} tracks, "
              f"{sum(len(t.refs) for t in tracks)} drop refs, "
              f"{sum(1 for t in tracks if len(t.refs))} with drops")

    if args.validate:
        # The gate. `d_fired` is what the shipped binary did on this very audio; the
        # replay must reproduce it tick for tick, not approximately.
        exact = mismatched = 0
        worst = []
        for tr in tracks:
            est = simulate(tr.t, tr.build, tr.loud, tr.sub, tr.sub_ref, DropCfg(),
                           tr.shipped_ticks)
            if len(est) == len(tr.fired) and np.allclose(est, tr.fired, atol=1e-6):
                exact += 1
            else:
                mismatched += 1
                worst.append((tr.tid, list(np.round(tr.fired, 2)), list(np.round(est, 2))))
        print(f"\nfidelity: {exact}/{len(tracks)} tracks reproduce the binary exactly")
        for tid, real, sim in worst[:10]:
            print(f"  MISMATCH {tid}: binary {real} vs replay {sim}")
        if mismatched:
            print("\nreplay has drifted — every sweep number below it would be a lie")
            return 1
        s = score(tracks, DropCfg())
        print("shipped: " + fmt(DropCfg(), s))
        if args.json:
            args.json.write_text(json.dumps({"fidelity": exact, "shipped": {
                k: v for k, v in s.items() if k != "per_track"}}, indent=2) + "\n")
        return 0

    if args.config:
        cfg = replace(DropCfg(), **json.loads(args.config))
        s = score(tracks, cfg)
        print(fmt(cfg, s))
        if args.json:
            args.json.write_text(json.dumps(
                {"config": cfg.__dict__, "score": {k: v for k, v in s.items()
                                                   if k != "per_track"}}, indent=2) + "\n")
        return 0

    if not args.stage:
        ap.error("need --validate, --stage or --config")

    shipped = score(tracks, DropCfg())
    print("shipped baseline: " + fmt(DropCfg(), shipped) + "\n")

    grid = stage_grid(args.stage) if args.stage != "combined" else combined_grid()
    print(f"stage {args.stage}: {len(grid)} configs")
    rows = []
    for i, cfg in enumerate(grid, 1):
        s = score(tracks, cfg)
        rows.append((cfg, s))
        if i % 25 == 0:
            print(f"  {i}/{len(grid)}", flush=True)

    keep = [(c, s) for c, s in rows
            if args.fa_budget is None or s["fa_per_min"] <= args.fa_budget]
    keep.sort(key=lambda cs: (-cs[1]["recall"], cs[1]["fa_per_min"]))
    print(f"\ntop {args.top} by recall"
          + (f" at FA/min <= {args.fa_budget}" if args.fa_budget else "")
          + f"  ({len(keep)}/{len(rows)} configs qualify):")
    spec = None
    if not args.specimens:
        try:
            spec = load_specimens(REPO / "bench" / "out" / "dropsweep" / "music",
                                  REPO / "bench" / "out" / "dropsweep" / "music_refs.json")
        except SystemExit:
            spec = None
    for cfg, s in keep[: args.top]:
        line = "  " + fmt(cfg, s)
        if spec:
            hv, hits, _ = specimen_detail(spec, cfg)
            line += (f"  | specimen {hv}/{len(HAND_VERIFIED)} hand-verified, "
                     f"{hits}/{sum(len(t.refs) for t in spec)} all")
        print(line)

    if args.json:
        args.json.write_text(json.dumps([
            {"config": c.__dict__, **{k: v for k, v in s.items() if k != "per_track"}}
            for c, s in rows], indent=2) + "\n")
        print(f"\nwrote {args.json}")
    return 0


def combined_grid() -> list[DropCfg]:
    """Arm levers crossed with the fire gate, around whatever the arm stage liked."""
    return [replace(DropCfg(), arm_buildup=lv, arm_sustain=su, arm_hold=ho,
                    loud_jump=j, baseline_ticks=bt)
            for lv, su, ho, j, bt in itertools.product(
                (0.4, 0.45, 0.5, 0.55),
                (2.0, 3.0, 4.0),
                (0.0, 2.0, 4.0),
                (0.06, 0.08, 0.12),
                (15, None))]


if __name__ == "__main__":
    sys.exit(main())
