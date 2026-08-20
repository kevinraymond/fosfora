#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Replay and sweep the drop state machine's arm/fire logic (#2211, workstream Q).

    bench/sweep_drop.py --validate            # gate: replay must reproduce the binary
    bench/sweep_drop.py --stage arm           # then sweep, tune half only
    bench/sweep_drop.py --stage combined --top 12
    bench/sweep_drop.py --local --gates       # candidate conjuncts, in-sample vs LOO (#2299)

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

`--gates` sweeps CANDIDATE conjuncts the machine does not read yet (#2299) — see the GATES
table. Two cautions that cost this round real time. First, a gate belongs INSIDE the state
machine, not applied to a fire list afterwards: killing a wrong fire also cancels the
refractory lockout it would have started, so a gate can raise recall, and post-hoc filtering
cannot see that. Second, read the leave-one-track-out column; at 23 drops a threshold fitted
on all nine tracks and scored on all nine reads about half a stop better than it is.
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
sys.path.insert(0, str(Path(__file__).resolve().parent))

from benchlib.metrics import CONVENTIONS, NEGATIVE_POLICIES  # noqa: E402

TICK_DT = 0.1
# One source of truth. This file re-implements the DETECTOR (it has to, to sweep it) but
# must not re-implement the SCORING RULE — the stale-DropCfg bug below is what two copies
# of one number costs. Imported at the price of benchlib's heavier deps, which uv caches.
_DROP = CONVENTIONS["drop"]
MATCH_BARS = _DROP["match_window_bars"]
LOCAL_S = _DROP["bar_local_window_s"]
FALLBACK_BAR_S = 2.0


def is_tune(track_id: str) -> bool:
    # Canonical split rule: analyze_predict_drop.py (sha256 parity).
    return int(hashlib.sha256(track_id.encode()).hexdigest(), 16) % 2 == 0


# =================================================================================
# The machine under test — a direct port of audio/structure.rs::update_drop
# =================================================================================


@dataclass(frozen=True)
class DropCfg:
    """Shipped defaults mirror `StructureConfig::default()` (audio/structure.rs).

    These had drifted a whole round behind it — .6/4.0/.08 against a shipped .40/3.0/.06
    — which is how #2259's refractory sweep came to vary one lever around the pre-rework
    arm config and measure nothing. Every grid now anchors on the dump's own meta line
    instead, so the drift cannot poison a sweep again; keeping these current only matters
    for a partial `--config`.
    """

    arm_buildup: float = 0.40
    arm_sustain: float = 3.0
    loud_jump: float = 0.06
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
    # --- candidate conjunct (#2299), which `update_drop` does not read ---
    # Name in GATES, or None for the shipped four-conjunct machine. See the GATES table
    # for what each form means and which ones are disqualified.
    gate: str | None = None
    gate_thr: float = 0.0


def simulate(t: np.ndarray, build: np.ndarray, loud: np.ndarray, sub: np.ndarray,
             sub_ref: np.ndarray, cfg: DropCfg, shipped_ticks: int = 141,
             gate_vals: np.ndarray | None = None) -> np.ndarray:
    """Return the times of fired drops. Mirrors update_drop tick for tick.

    The arm timer accumulates in **float32**, because the detector does and the rounding
    is load-bearing: at the one true positive in the specimen set the f32 sum is still a
    hair under the 4.0 s requirement on the tick f64 has already crossed it, so an f64
    replay fires a tick early. The margin there is 0.1 s of a 4.0 s threshold — the
    shipped detector arms that close to the edge.

    `gate_vals` is an optional fifth conjunct, AND-ed in. It can only ever suppress a fire —
    but suppressing one can *create* a later fire, and recall is NOT monotone in the
    threshold as a result: the refractory is armed by whichever fire happens first, so
    killing a wrong fire cancels a 16 s lockout that was masking a real drop behind it.
    Measured, at the loose arm with kick_dens_delta: Psykovsky fires at 28.6 s and locks out
    the 38.5 s drop; gate the 28.6 away and it hits at 39.0. Same shape on the Tiesto at
    176.2 s masking 184.7 s. Anything that reasons about a gate by filtering an existing
    fire list instead of replaying the machine will miss this and understate every gate.

    `gate_vals is None` reproduces the shipped machine exactly, which is what `--validate`
    checks.
    """
    f32 = np.float32
    high = f32(0.0)
    dt = f32(TICK_DT)
    decay = f32(cfg.arm_decay) * dt
    level = f32(cfg.arm_buildup)
    sustain = f32(cfg.arm_sustain)
    gate = f32(cfg.loud_jump)
    sub_frac = f32(cfg.subbass_return)
    gate_thr = cfg.gate_thr
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

        # NaN abstains rather than blocks: the only ticks a window comes back empty on are
        # the first few of a track, and a gate with no evidence must not manufacture a
        # difference from the shipped machine there.
        passes = gate_vals is None or not (gate_vals[i] < gate_thr)

        if armed and now >= refractory_until and jump >= gate and sub_returning and passes:
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


# Schema v3 (#2299): candidate conjuncts the drop machine does not read yet. Optional
# everywhere, because the Harmonix sidecar corpus is v2 and is not being re-dumped —
# an analysis that needs them asks `Track.trio()` and gets a loud failure if it is on
# the wrong corpus, rather than every v2 reader breaking at load.
V3_COLS = ("d_kick", "d_perc", "d_hratio")


# =================================================================================
# Candidate conjuncts (#2299)
# =================================================================================

# A tick counts as carrying a kick transient above this. `kick` is a 0..1 detector output,
# not a level — see the saturation note on kick_pm1 for why counting transients and reading
# heights are very different measurements of it.
KICK_PRESENT = 0.5


def _win(t: np.ndarray, v: np.ndarray, lo: float, hi: float, op: str) -> np.ndarray:
    """Per-tick reduction of `v` over the window [t+lo, t+hi], in seconds of real time.

    Bounds come from `searchsorted` on the tick times, not from an index offset: the
    sidecar's ticks land every ~0.107 s rather than the 0.100 s TICK_DT names, so a window
    counted in ticks would run ~7% long — the same class of mistake as the baseline ring
    that was sized at the analysis rate and pushed at the tick rate (#2212).
    """
    a = np.searchsorted(t, t + lo, side="left")
    b = np.searchsorted(t, t + hi, side="right")
    out = np.empty(len(t), dtype=np.float64)
    for i in range(len(t)):
        seg = v[a[i]:b[i]]
        if len(seg) == 0:
            out[i] = np.nan
        elif op == "max":
            out[i] = seg.max()
        elif op == "mean":
            out[i] = seg.mean()
        else:  # "dens"
            out[i] = float((seg > KICK_PRESENT).mean())
    return out


def _ratio(t, v, lo, hi, rlo, rhi):
    return _win(t, v, lo, hi, "mean") / np.maximum(1e-9, _win(t, v, rlo, rhi, "mean"))


# form -> (causal, doc, builder). CAUSAL is what decides whether a form could ever ship:
# `update_drop` fires on the tick it decides, so a window extending past `t` describes a cue
# the detector could only emit late. Kevin ruled causal-only on 2026-08-20 (#2299). The
# disqualified and refuted forms stay in the table anyway — reproducing a refutation needs
# the refuted form itself, not a lookalike, and a later round that rediscovers one of these
# should land on the measurement rather than on the idea.
GATES: dict[str, tuple[bool, str, object]] = {
    "kick_pm1": (
        False,
        "REFUTED (#2360, withdrawn 2026-08-20). Max kick within +-1 s. `kick` is Passthrough, "
        "self-normalized by its own long-term P95 (audio/schema.rs, A3 #1454), so its windowed "
        "max pins to the ceiling wherever a kick is playing at all: at the loose arm 33 of 43 "
        "wrong fires and 9 of 10 right fires read exactly 1.000. Kills 5 of 43 there, worse "
        "than the 6 of 21 it managed at the incumbent — widening the arm drags the fitted "
        "threshold off the ceiling and the gate collapses.",
        lambda t, x: _win(t, x["d_kick"], -1.0, 1.0, "max"),
    ),
    "kick_trail1": (
        True,
        "Causal form of kick_pm1: max kick over the trailing 1 s. Saturates for the same "
        "reason and is here as the control's control.",
        lambda t, x: _win(t, x["d_kick"], -1.0, 0.0, "max"),
    ),
    "kick_dens_delta": (
        True,
        "Best causal form measured this round: how much denser kick transients are over the "
        "trailing 2 s than over the 6 s before that. Immune to the saturation that killed "
        "kick_pm1, because it counts ticks carrying a transient instead of reading a "
        "normalized height. Still lands under the incumbent's precision under LOO.",
        lambda t, x: (_win(t, x["d_kick"], -2.0, 0.0, "dens")
                      - _win(t, x["d_kick"], -8.0, -2.0, "dens")),
    ),
    "perc_ratio": (
        True,
        "Percussive energy over the trailing 1 s against the 6 s ending 2 s ago. Kills ZERO "
        "of 43 wrong fires at any zero-recall-cost threshold: the information that separates "
        "a drop from a build-up arrives after the decision point, which is the whole content "
        "of the causal-only result.",
        lambda t, x: _ratio(t, x["d_perc"], -1.0, 0.0, -8.0, -2.0),
    ),
    "perc_post4_ratio": (
        False,
        "DISQUALIFIED by causal-only, recorded because it is the strongest gate measured: "
        "percussive energy over 0..+4 s against -6..-2 s. Kills 24 of 43 in-sample and is the "
        "only form anywhere that beats the incumbent on precision under LOO (.304/.280 vs "
        "the incumbent .304/.250). The price is a drop cue arriving four seconds late.",
        lambda t, x: _ratio(t, x["d_perc"], 0.0, 4.0, -6.0, -2.0),
    ),
}


class Track:
    __slots__ = ("tid", "t", "build", "loud", "sub", "sub_ref", "fired",
                 "refs", "negs", "beats", "downbeats", "duration", "shipped_ticks",
                 "dump_cfg", "v3", "_gates")

    def trio(self) -> dict[str, np.ndarray]:
        """The v3 columns, or a hard failure naming the track that lacks them."""
        if self.v3 is None:
            sys.exit(f"{self.tid}: schema v2 sidecar (no {V3_COLS[0]}) — re-dump with the "
                     f"v3 binary before asking for the HPSS trio")
        return self.v3

    def gate(self, form: str) -> np.ndarray:
        """Per-tick value of candidate conjunct `form`. Computed once, then cached.

        The value does not depend on DropCfg — only the threshold does — so a 192-config
        grid pays for each track's windows exactly once.
        """
        if form not in self._gates:
            if form not in GATES:
                sys.exit(f"unknown gate {form!r} — one of {', '.join(GATES)}")
            self._gates[form] = GATES[form][2](self.t, self.trio())
        return self._gates[form]

    def __init__(self, tid: str, records: list[dict], ann: dict, meta: dict | None = None):
        self.dump_cfg = (meta or {}).get("cfg", {})
        self._gates: dict[str, np.ndarray] = {}
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
        self.v3 = (
            {k: np.array([r[k] for r in records], dtype=np.float64) for k in V3_COLS}
            if all(k in records[0] for k in V3_COLS)
            else None
        )
        self.refs = np.array([d["time"] for d in ann.get("drops", [])], dtype=np.float64)
        self.negs = np.array([d["time"] for d in ann.get("not_drops") or []],
                             dtype=np.float64)
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
        parsed = [json.loads(line) for line in p.read_text().splitlines() if line.strip()]
        meta = next((r for r in parsed if "meta" in r), {})
        records = [r for r in parsed if "meta" not in r]
        if not records or "d_fired" not in records[0]:
            sys.exit(f"{p}: schema v1 sidecar — re-dump")
        out.append(Track(name, records,
                         {"drops": ref["drops"], "audio": {"duration_s": ref["duration_s"]}},
                         meta))
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
        est = simulate_track(tr, cfg)
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


# The local corpus has no sha256 parity split — it is nine tracks Kevin owns, not a
# dataset — so the split is named explicitly in a frozen manifest and read from there.
# Without this, labelling the holdout tracks would silently enlarge the tune corpus,
# because load_local globs the whole directory.
LOCAL_SPLIT = REPO / "bench" / "manifests" / "q_drop_precision_frozen.json"


def local_split(which: str) -> set[str] | None:
    """Track ids in the named half, or None for 'all' / no manifest yet."""
    if which == "all" or not LOCAL_SPLIT.exists():
        return None
    m = json.loads(LOCAL_SPLIT.read_text())
    ids = m.get("split", {}).get(which)
    if ids is None:
        sys.exit(f"{LOCAL_SPLIT.name} has no split.{which} — one of "
                 f"{', '.join(m.get('split', {}))}")
    return set(ids)


def load_local(sidecar_dir: Path, labels_dir: Path, which: str = "tune") -> list[Track]:
    """Kevin's own drop-shaped EDM, labelled by ear (#2299, Phase 2).

    Unlike Harmonix these bundles carry `not_drops` — fires he was shown and ruled wrong —
    so `score` applies the beat_grace policy here and nowhere else. They carry no beat
    annotation, so the +-1 bar tolerance falls back to 2 s (measured: real per-track bar
    length, 99.9-170.3 BPM across the 9, flips no match verdict).

    `which` selects a half of the frozen split. It defaults to `tune` so that constant
    selection cannot read the holdout by forgetting a flag — the failure has to be typing
    --split holdout on purpose.
    """
    keep = local_split(which)
    out = []
    for b in sorted(labels_dir.glob("*.json")):
        ann = json.loads(b.read_text())
        # Not every .json dropped in here is an annotation bundle — a --fires seed lands
        # as one and used to crash the loader on a bare KeyError. Skip by schema, loudly,
        # rather than either crashing or quietly scoring a corpus short of a track.
        if not str(ann.get("schema", "")).startswith("fosfora-bench-annotation/"):
            print(f"  (skip {b.name}: not an annotation bundle)")
            continue
        if keep is not None and ann["track_id"] not in keep:
            continue
        p = sidecar_dir / f"{ann['track_id']}.jsonl"
        if not p.exists():
            sys.exit(f"{b.name}: no sidecar at {p} — run dump_structure_sidecar.py --files")
        parsed = [json.loads(line) for line in p.read_text().splitlines() if line.strip()]
        meta = next((r for r in parsed if "meta" in r), {})
        records = [r for r in parsed if "meta" not in r]
        if not records or "d_fired" not in records[0]:
            sys.exit(f"{p}: schema v1 sidecar — re-dump")
        out.append(Track(ann["track_id"], records, ann, meta))
    if not out:
        sys.exit(f"no label bundles in {labels_dir}"
                 + (f" for split {which!r}" if keep is not None else ""))
    if keep is not None:
        unlabelled = sorted(keep - {t.tid for t in out})
        if unlabelled:
            # Loud, because a half that is only half-labelled scores as if the missing
            # tracks contributed nothing — which reads as a corpus, not as a gap.
            print(f"  NOTE: {len(unlabelled)} track(s) in split {which!r} are not labelled "
                  f"yet: {', '.join(t[:28] for t in unlabelled)}")
    return out


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
        records, meta = [], {}
        for line in p.read_text().splitlines():
            if not line.strip():
                continue
            r = json.loads(line)
            if "meta" in r:
                meta = r
                continue
            if "d_fired" not in r:
                v1 += 1
                records = []
                break
            records.append(r)
        if len(records) < 100:
            continue
        out.append(Track(tid, records, json.loads(ann_path.read_text()), meta))
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


# The negative policy, read from benchlib rather than restated — see CONVENTIONS["drop"]
# and NEGATIVE_POLICIES for what each mode means and why `strict` is the default.
#
# One deliberate difference from benchlib: there, coincidence with a recorded negative is
# tested at +-0.25 s, because the fires being scored are the shipped binary's and a
# rejection is stamped on the very fire it judged. In a SWEEP the fires move, so a negative
# has to mean a rejected *moment* — +-1 bar, the same zone of influence a drop gets.
GRACE_BEATS = _DROP["negative_override_beats"]


def _allowed(tr: Track, est_t: float, ref_t: float, bar: float) -> bool:
    policy = _DROP["negative_policy"]
    if policy not in NEGATIVE_POLICIES:
        sys.exit(f"negative_policy {policy!r} is not one of {NEGATIVE_POLICIES}")
    if policy == "bar_window" or not len(tr.negs):
        return True
    if not np.any(np.abs(tr.negs - est_t) <= MATCH_BARS * bar):
        return True
    if policy == "strict":
        return False
    return abs(est_t - ref_t) <= GRACE_BEATS * (bar / 4.0)


def match(tr: Track, est: np.ndarray) -> tuple[set[int], set[int]]:
    """Greedy one-to-one by ascending |dt|, +-1 bar measured locally.

    The single implementation of the matching rule: `score` counts with it and the gate
    fitter labels fires right-or-wrong with it. Two copies of one number is what the stale
    DropCfg cost this round already.
    """
    pairs = []
    for ri, rt in enumerate(tr.refs):
        bar = bar_seconds(tr, float(rt))
        tol = MATCH_BARS * bar
        for ei, et in enumerate(est):
            if abs(et - rt) <= tol and _allowed(tr, et, rt, bar):
                pairs.append((abs(et - rt), ri, ei))
    pairs.sort()
    used_r, used_e = set(), set()
    for _, ri, ei in pairs:
        if ri in used_r or ei in used_e:
            continue
        used_r.add(ri)
        used_e.add(ei)
    return used_r, used_e


def simulate_track(tr: Track, cfg: DropCfg) -> np.ndarray:
    """`simulate` with this track's own ring capacity and gate array wired in."""
    return simulate(tr.t, tr.build, tr.loud, tr.sub, tr.sub_ref, cfg, tr.shipped_ticks,
                    tr.gate(cfg.gate) if cfg.gate else None)


def aggregate(fired: list[tuple[Track, np.ndarray]]) -> dict:
    """Corpus totals from per-track (track, fired times). The one place the metric lives."""
    hits = misses = false = 0
    total_min = 0.0
    per_track = []
    for tr, est in fired:
        used_r, used_e = match(tr, est)
        h = len(used_r)
        hits += h
        misses += len(tr.refs) - h
        false += len(est) - len(used_e)
        total_min += tr.duration / 60.0
        per_track.append((tr.tid, h, len(tr.refs), len(est) - len(used_e)))
    tracks = [tr for tr, _ in fired]
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


def score(tracks: list[Track], cfg: DropCfg) -> dict:
    return aggregate([(tr, simulate_track(tr, cfg)) for tr in tracks])


def fit_gate(tracks: list[Track], cfg: DropCfg, form: str, quantile: float = 0.0) -> float:
    """The gate threshold that keeps every right fire on `tracks` (quantile 0 = zero cost).

    Fitted on fires produced with the gate OFF: the gate is the thing being chosen, so
    fitting it on its own filtered output would be circular. Above quantile 0 it starts
    trading right fires away, which the caller has to be willing to pay for.

    `-inf` when the fit set produced no right fire at all — an inert gate, which is the
    only honest answer with nothing to fit on.
    """
    vals = []
    ungated = replace(cfg, gate=None)
    for tr in tracks:
        est = simulate_track(tr, ungated)
        _, matched = match(tr, est)
        g = tr.gate(form)
        for ei in sorted(matched):
            vals.append(float(g[int(np.searchsorted(tr.t, est[ei]))]))
    vals = [v for v in vals if not np.isnan(v)]
    if not vals:
        return -np.inf
    return float(np.quantile(vals, quantile))


def score_loo(tracks: list[Track], cfg: DropCfg, form: str, quantile: float = 0.0) -> dict:
    """Leave-one-track-out: fit the gate threshold on the other tracks, score the held-out one.

    With 23 drops across 9 tracks there is no honest split, and this is the most that can be
    claimed. It is not a formality — fitting on all nine and scoring all nine reported .345
    precision for the strongest form this round, where LOO reports .280. The gap is the size
    of the lie a threshold fitted to its own test set tells at this n.
    """
    out = []
    for held in tracks:
        thr = fit_gate([t for t in tracks if t.tid != held.tid], cfg, form, quantile)
        out.append((held, simulate_track(held, replace(cfg, gate=form, gate_thr=thr))))
    return aggregate(out)


def cfg_from_dump(tr: Track) -> DropCfg:
    """The config the binary held when this sidecar was written.

    Fields the writer did not record fall back to DropCfg's defaults, which is correct for
    the schema-v2 corpus: it predates drop_arm_hold and drop_baseline_seconds, and those
    defaults reproduce the behaviour it was dumped with.
    """
    m = {"drop_arm_buildup": "arm_buildup", "drop_arm_sustain": "arm_sustain",
         "drop_loud_jump": "loud_jump", "drop_subbass_return": "subbass_return",
         "drop_refractory": "refractory", "drop_arm_hold": "arm_hold"}
    over = {m[k]: float(v) for k, v in tr.dump_cfg.items() if k in m}
    if "drop_baseline_seconds" in tr.dump_cfg:
        over["baseline_ticks"] = int(round(float(tr.dump_cfg["drop_baseline_seconds"]) * 10.0))
    return replace(DropCfg(), **over)


def fmt(cfg: DropCfg, s: dict) -> str:
    gate = f"gate {cfg.gate}>={cfg.gate_thr:.3f} " if cfg.gate else ""
    return (f"lvl {cfg.arm_buildup:.2f} sus {cfg.arm_sustain:4.1f} hold {cfg.arm_hold:4.1f} "
            f"dec {cfg.arm_decay:.1f} jump {cfg.loud_jump:.3f} "
            f"base {cfg.baseline_ticks if cfg.baseline_ticks is not None else 'shipped':>7} "
            f"sub {cfg.subbass_return:.2f} refr {cfg.refractory:4.1f} {gate}| "
            f"recall {s['recall']:.3f} ({s['hits']}/{s['n_refs']})  "
            f"prec {s['precision']:.3f}  FA/min {s['fa_per_min']:.3f}  "
            f"est/track {s['est_per_track']:.2f}")


# =================================================================================
# Stages
# =================================================================================


def stage_grid(stage: str, base: DropCfg) -> list[DropCfg]:
    """`base` is the config the corpus was dumped under, never DropCfg()'s defaults.

    #2259's refractory sweep measured nothing because it varied one lever around the
    *pre-rework* arm config, where the machine fired once in 77 refs and every row came
    out identical. Anchoring the grid on the dump's own meta line makes that failure
    impossible rather than unlikely.
    """
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
        # Extended below 8 s for #2299: 6 of 13 missed drops on Kevin's corpus are
        # swallowed by the lockout an earlier fire started, so the interesting range is
        # shorter than anything #2259's corrected sweep could see on Harmonix.
        return [replace(base, refractory=r)
                for r in (2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0)]
    sys.exit(f"unknown stage {stage}")


def write_fires(tracks: list[Track], cfg: DropCfg, path: Path) -> None:
    """`{track_id: [times]}` for `label_drops.py --predictions` (#2365).

    Carries the config it came from in a `_config` key, because a bundle labelled against
    these fires is evidence about THAT config and a later reader has to be able to tell
    which. The loader skips underscore keys.
    """
    out: dict[str, object] = {"_config": {k: v for k, v in cfg.__dict__.items()}}
    n = 0
    for tr in tracks:
        est = [round(float(t), 3) for t in simulate_track(tr, cfg)]
        out[tr.tid] = est
        n += len(est)
    path.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {n} fires across {len(tracks)} tracks -> {path}")


def report_gates(tracks: list[Track], args) -> int:
    """Every candidate conjunct against two arm configs, in-sample and LOO (#2299).

    Two configs and not one, because the whole reason a free conjunct is interesting is that
    it might make a LOOSER arm payable — #2360 measured the kick gate only at the incumbent,
    where there are 21 wrong fires to kill rather than 43, and that understates a real gate
    while flattering a saturating one.

    Read the LOO column, not the in-sample one. The in-sample column is printed only so the
    size of the gap between them stays visible.
    """
    incumbent = cfg_from_dump(tracks[0])
    loose = replace(incumbent, arm_buildup=0.40, arm_sustain=2.0, arm_hold=8.0, arm_decay=1.0)
    rows = []
    for label, cfg in (("incumbent", incumbent), ("loose arm", loose)):
        base = score(tracks, cfg)
        judged = sum(
            1 for tr in tracks for et in simulate_track(tr, cfg)
            if (len(tr.refs) and np.any(np.abs(tr.refs - et) <= MATCH_BARS * bar_seconds(tr, float(et))))
            or (len(tr.negs) and np.any(np.abs(tr.negs - et) <= MATCH_BARS * bar_seconds(tr, float(et))))
        )
        n_fires = base["hits"] + base["false"]
        print(f"\n=== {label}: " + fmt(cfg, base))
        # #2365: a labelling pass seeded from one config only adjudicates that config's
        # fires. Any row firing more than the seed config is scoring its own unlabelled
        # moments as errors, so the unjudged count is part of reading the precision.
        print(f"    {n_fires} fires, {judged} adjudicated by ear, "
              f"{n_fires - judged} at moments NEVER JUDGED"
              + ("  <-- precision below is partly assumption" if n_fires > judged else ""))
        print(f"    {'gate':20}{'causal':>8}{'thr':>9}{'in-sample':>22}{'leave-one-track-out':>24}")
        for form, (causal, _doc, _fn) in GATES.items():
            thr = fit_gate(tracks, cfg, form, args.gate_quantile)
            ins = score(tracks, replace(cfg, gate=form, gate_thr=thr))
            loo = score_loo(tracks, cfg, form, args.gate_quantile)
            rows.append({"config": label, "gate": form, "causal": causal, "thr": thr,
                         "in_sample": {k: v for k, v in ins.items() if k != "per_track"},
                         "loo": {k: v for k, v in loo.items() if k != "per_track"}})
            print(f"    {form:20}{'yes' if causal else 'NO':>8}{thr:9.3f}"
                  f"{ins['recall']:9.3f} /{ins['precision']:7.3f}"
                  f"{'':6}{loo['recall']:9.3f} /{loo['precision']:7.3f}"
                  + ("" if causal else "   [disqualified]"))
        print(f"    {'(no gate)':20}{'':>8}{'':>9}"
              f"{base['recall']:9.3f} /{base['precision']:7.3f}")
    if args.json:
        args.json.write_text(json.dumps(rows, indent=2) + "\n")
        print(f"\nwrote {args.json}")
    return 0


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
    ap.add_argument("--local", action="store_true",
                    help="score Kevin's hand-labelled tracks (bench/labels/) instead of "
                         "a dataset — the only corpus carrying recorded negatives")
    ap.add_argument("--split", choices=("tune", "holdout", "all"), default="tune",
                    help="which half of the frozen local split to score (default tune). "
                         "The holdout is measured ONCE, at the end of the round")
    ap.add_argument("--allow-partial", action="store_true",
                    help="score only the v2 sidecars present, reporting what was skipped")
    ap.add_argument("--gates", action="store_true",
                    help="report every candidate conjunct in GATES against this corpus, "
                         "in-sample and leave-one-track-out (#2299)")
    ap.add_argument("--gate-quantile", type=float, default=0.0,
                    help="quantile of the right fires the gate threshold is fitted at; "
                         "0 = keep every right fire on the fit set")
    ap.add_argument("--fires", type=Path, default=None,
                    help="write {track_id: [times]} for the scored config, to seed a "
                         "labelling pass with `label_drops.py --predictions` (#2365). "
                         "Write it to bench/manifests/, NOT bench/labels/ — that directory "
                         "is globbed for annotation bundles")
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    if args.local:
        tracks = load_local(REPO / "bench" / "out" / "dropsweep" / "music",
                            REPO / "bench" / "labels", args.split)
        print(f"local[{args.split}]: {len(tracks)} tracks, "
              f"{sum(len(t.refs) for t in tracks)} drops, "
              f"{sum(len(t.negs) for t in tracks)} recorded negatives")
    elif args.specimens:
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
        # The gate. `d_fired` is what the binary did on this very audio; the replay must
        # reproduce it tick for tick, not approximately.
        #
        # Replayed under the config recorded in each dump's meta line, NOT under today's
        # defaults — otherwise this fails the moment a round changes a constant, and reads
        # as replay drift when it is only a stale corpus.
        exact = mismatched = 0
        worst = []
        for tr in tracks:
            # cfg_from_dump never carries a gate, so this is the shipped four-conjunct
            # machine — which is the point: --validate proves the port is still the binary,
            # and a gate in the loop would be proving something else.
            est = simulate_track(tr, cfg_from_dump(tr))
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
        dumped = cfg_from_dump(tracks[0])
        s = score(tracks, dumped)
        print("as dumped: " + fmt(dumped, s))
        if args.json:
            args.json.write_text(json.dumps({"fidelity": exact, "shipped": {
                k: v for k, v in s.items() if k != "per_track"}}, indent=2) + "\n")
        return 0

    if args.config:
        cfg = replace(DropCfg(), **json.loads(args.config))
        s = score(tracks, cfg)
        print(fmt(cfg, s))
        if args.fires:
            write_fires(tracks, cfg, args.fires)
        if args.json:
            args.json.write_text(json.dumps(
                {"config": cfg.__dict__, "score": {k: v for k, v in s.items()
                                                   if k != "per_track"}}, indent=2) + "\n")
        return 0

    if args.gates:
        return report_gates(tracks, args)

    if not args.stage:
        ap.error("need --validate, --stage, --config or --gates")

    # The incumbent is whatever produced this corpus, not whatever the defaults say today.
    # It anchors every grid below, so a corpus dumped under two configs would silently
    # sweep around one of them: refuse instead.
    configs = {json.dumps(t.dump_cfg, sort_keys=True) for t in tracks}
    if len(configs) > 1:
        sys.exit(f"corpus was dumped under {len(configs)} different configs — re-dump it "
                 f"under one before sweeping")
    incumbent = cfg_from_dump(tracks[0])
    print("as-dumped baseline: " + fmt(incumbent, score(tracks, incumbent)) + "\n")

    grid = (stage_grid(args.stage, incumbent) if args.stage != "combined"
            else combined_grid(incumbent))
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


def combined_grid(base: DropCfg) -> list[DropCfg]:
    """Arm levers crossed with the fire gate, around whatever the arm stage liked."""
    return [replace(base, arm_buildup=lv, arm_sustain=su, arm_hold=ho,
                    loud_jump=j, baseline_ticks=bt)
            for lv, su, ho, j, bt in itertools.product(
                (0.4, 0.45, 0.5, 0.55),
                (2.0, 3.0, 4.0),
                (0.0, 2.0, 4.0),
                (0.06, 0.08, 0.12),
                (15, None))]


if __name__ == "__main__":
    sys.exit(main())
