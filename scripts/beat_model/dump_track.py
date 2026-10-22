#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Run Fosfora over one audio file with the hop sidecar, the beat sidecar and the signal
dump, and store everything per hop in one npz.

    dump_track.py --bin <fosfora> --audio song.mp3 --out raw/fma/012345.npz [--keep-jsonl]

The binary needs `--features analyze` (`--signal-dump`). Nothing is kept but the npz: the
hop sidecar (~1 KB per hop) and the beat sidecar are deleted after conversion unless
`--keep-jsonl`. Hop h is the analysis frame ending at sample (h + 1) * hop;
ts = (h + 1) * hop / sr.

npz contents:
  ts, silent, kick_prob, kf, sf, pn, act per hop, raw, every hop (see hopdump.py; act is the
                                         runtime's beat activation, NaN from a v1 dump);
  kf_names, sf_names, pn_names           column names
  beats          float64  event times of the beats the grid fired (the hop sidecar's `bt`,
                          cross-checked against the signal dump's /beat events)
  downbeats, onsets, kicks, drops        float64  event times from the signal dump
  sc_ts, sc_mode, sc_<field>...          one row per tempo update (beat sidecar); mode is an
                                         index into MODES
  ev_ts, ev_name, ev_str                 beat sidecar state transitions
  meta           json string: binary + sha256, audio + sha256, sr, hop count, duration,
                 event counts, dump seconds
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import hopdump  # noqa: E402

PREFIX = "/fosfora/v1/"
EVENT_ADDRS = {
    PREFIX + "beat": "beats_dump",
    PREFIX + "downbeat": "downbeats",
    PREFIX + "onset": "onsets",
    PREFIX + "stem/drums/onset": "kicks",
    PREFIX + "drop": "drops",
}
U_KEYS = ["raw_bpm", "conf", "comb", "peak", "floor", "c2", "c3", "c4", "meas_bpm", "filt_bpm",
          "support", "locked", "seeded", "anchor_bpm", "challenge", "pub_bpm", "on_sup",
          "kick_sup", "grid_n"]
MODES = ["none", "acq", "muted", "locked", "other"]


def sha256_file(path, chunk=1 << 20):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            b = f.read(chunk)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def read_beat_sidecar(path: Path):
    sc = {k: [] for k in ["ts", "mode"] + U_KEYS}
    ev_ts, ev_name, ev_str = [], [], []
    if path.exists():
        with open(path, encoding="utf-8") as f:
            for line in f:
                d = json.loads(line)
                if d["k"] == "u":
                    sc["ts"].append(d["ts"])
                    sc["mode"].append(MODES.index(d["mode"]) if d["mode"] in MODES else MODES.index("other"))
                    for k in U_KEYS:
                        v = d.get(k)
                        sc[k].append(np.nan if v is None else float(v))
                elif d["k"] == "e":  # "w" / "a" are the arbiter's window and votes (#4234)
                    ev_ts.append(d["ts"])
                    ev_name.append(d["ev"])
                    ev_str.append(float(d.get("str", np.nan)))
    return sc, ev_ts, ev_name, ev_str


def run(bin_path: Path, audio: Path, out: Path, keep_jsonl: bool = False, nice: int = 19) -> dict:
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    hop_jsonl = out.with_suffix(".hops.jsonl")
    beat_jsonl = out.with_suffix(".beat.jsonl")
    for p in (hop_jsonl, beat_jsonl):
        if p.exists():  # a sidecar refuses to overwrite data; a previous failed run may have left one
            p.unlink()
    env = dict(os.environ, FOSFORA_HOP_SIDECAR=str(hop_jsonl), FOSFORA_BEAT_SIDECAR=str(beat_jsonl),
               RUST_LOG="error")
    cmd = [str(bin_path), "--signal-dump", str(audio), "--out", "-"]
    if nice:
        cmd = ["nice", "-n", str(nice)] + cmd
    t0 = time.time()
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env,
                            text=True, bufsize=1 << 16)
    events = {v: [] for v in EVENT_ADDRS.values()}
    meta_line = None
    for line in proc.stdout:
        if line.startswith('{"addr":"/fosfora/v1/feat/'):
            continue
        d = json.loads(line)
        if "meta" in d:
            meta_line = d
            continue
        key = EVENT_ADDRS.get(d["addr"])
        if key is not None:
            events[key].append(d["ts"])
    _, err = proc.communicate()
    if proc.returncode != 0:
        raise RuntimeError(f"{bin_path} exited {proc.returncode} on {audio}: {err.strip()[-400:]}")
    if meta_line is None:
        raise RuntimeError(f"no meta line from {bin_path} on {audio}")
    dump_s = time.time() - t0

    d = hopdump.read(hop_jsonl)
    # The hop sidecar's beat times and the signal dump's /beat events are the same beats
    # seen from two places; they must agree, or one of the two writers is off.
    bd = np.asarray(events.pop("beats_dump"), np.float64)
    if len(bd) != len(d.beat_times) or (len(bd) and np.abs(bd - d.beat_times).max() > 2e-3):
        raise RuntimeError(f"{audio}: {len(d.beat_times)} sidecar beats vs {len(bd)} dump beats")
    sc, ev_ts, ev_name, ev_str = read_beat_sidecar(beat_jsonl)

    meta = dict(
        binary=str(bin_path), binary_sha256=sha256_file(bin_path),
        audio=str(audio), audio_sha256=sha256_file(audio),
        sr=d.sr, hop=d.hop, hop_hz=d.hop_hz, n_hops=d.n_hops, duration_s=d.duration, hop_format=d.version,
        silent_hops=int(d.silent.sum()), n_beats=len(d.beat_times),
        counts={k: len(v) for k, v in events.items()},
        n_tempo_updates=len(sc["ts"]), n_events=len(ev_ts),
        dump_cmd=" ".join(cmd), dump_seconds=round(dump_s, 1),
        sample_rate=meta_line["sample_rate"], hop_sidecar_bytes=hop_jsonl.stat().st_size,
    )
    arrays = dict(
        ts=d.ts, silent=d.silent, kick_prob=d.kick_prob, kf=d.kf, sf=d.sf, pn=d.pn, act=d.act,
        kf_names=np.array(d.kf_names), sf_names=np.array(d.sf_names), pn_names=np.array(d.pn_names),
        beats=d.beat_times,
        **{k: np.asarray(v, np.float64) for k, v in events.items()},
        **{f"sc_{k}": np.asarray(v, np.float64) for k, v in sc.items()},
        ev_ts=np.asarray(ev_ts, np.float64), ev_name=np.array(ev_name, dtype=str),
        ev_str=np.asarray(ev_str, np.float64), meta=np.array(json.dumps(meta)),
    )
    tmp = out.with_suffix(".part.npz")
    np.savez_compressed(tmp, **arrays)
    os.replace(tmp, out)
    if not keep_jsonl:
        hop_jsonl.unlink(missing_ok=True)
        beat_jsonl.unlink(missing_ok=True)
    return meta


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--bin", type=Path, default=Path(os.environ.get("FOSFORA_BIN", "target/release/fosfora")))
    ap.add_argument("--audio", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--keep-jsonl", action="store_true")
    ap.add_argument("--force", action="store_true")
    a = ap.parse_args()
    if a.out.exists() and not a.force:
        print(f"skip {a.out} (exists)")
        return 0
    meta = run(a.bin, a.audio, a.out, keep_jsonl=a.keep_jsonl)
    print(f"{a.out.name}: {meta['n_hops']} hops ({meta['duration_s']:.0f} s), {meta['n_beats']} beats, "
          f"{meta['silent_hops']} silent, {meta['n_tempo_updates']} tempo updates, {meta['dump_seconds']} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
