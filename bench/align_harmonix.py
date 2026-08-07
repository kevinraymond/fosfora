#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["librosa", "numpy", "soundfile"]
# ///
"""The Harmonix alignment gate: is our YouTube re-fetch the annotated audio?

    bench/align_harmonix.py run [--only ID] [--limit N] [--force]
    bench/align_harmonix.py report

Re-fetched YouTube audio can be a different master, a different edit, or the
right recording shifted by an encoder/CDN constant. Three stages, cheap to
expensive, thresholds from manifests/harmonix.json:gate:

  1. duration pre-check vs metadata.csv (±1 s) — catches different edits;
  2. offset estimation: normalized cross-correlation between an impulse train
     built from the JAMS onsets (librosa onsets of the ORIGINAL audio's first
     30 s, shipped by the authors for exactly this) and the fetched audio's
     onset-strength envelope, ±5 s window on a 10 ms grid;
  3. whole-track verification: DTW between the authors' distributed mel
     spectrogram of the original and ours of the fetched audio (their
     parameters, from info.json) — the path must be a straight line of slope
     ~1 through the xcorr offset, or the match is only local (mid-track edits,
     speed changes).

Verdicts: pass (|offset| <= 25 ms) / pass_with_offset (constant, <= 2 s;
prep shifts annotations by it) / reject:<stage>. Everything lands in
status.json["alignment"]; rejects are listed, never silently dropped.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import librosa
import numpy as np

sys.path.insert(0, str(Path(__file__).parent))

import datasetlib as dl
import fetch_harmonix as fh

GRID_S = 0.010  # xcorr grid
JAMS_SPAN_S = 35.0  # the JAMS onsets cover the original's first ~30 s


def load_mel_info(mel_dir: Path) -> dict:
    for cand in mel_dir.glob("**/info.json"):
        with cand.open(encoding="utf-8") as f:
            return json.load(f)
    raise dl.DatasetError(f"no info.json under {mel_dir} — melspecs not fetched?")


def find_melspec(mel_dir: Path, file_id: str) -> Path | None:
    for ext in (".npy", ".npz"):
        hits = list(mel_dir.glob(f"**/{file_id}*{ext}"))
        if hits:
            return hits[0]
    return None


def load_melspec(path: Path) -> np.ndarray:
    if path.suffix == ".npz":
        z = np.load(path)
        arr = z[z.files[0]]
    else:
        arr = np.load(path)
    arr = np.asarray(arr)
    if arr.ndim != 2:
        raise ValueError(f"{path.name}: expected 2-D mel, got {arr.shape}")
    # orient as (mels, frames): mel axis is the smaller one in practice
    if arr.shape[0] > arr.shape[1]:
        arr = arr.T
    return arr.astype(np.float32)


def jams_onsets(repo: Path, file_id: str) -> np.ndarray:
    with (repo / "dataset" / "jams" / f"{file_id}.jams").open(encoding="utf-8") as f:
        j = json.load(f)
    for a in j.get("annotations", []):
        if a.get("namespace") == "onset":
            return np.array([d["time"] for d in a["data"]], dtype=np.float64)
    return np.array([], dtype=np.float64)


def impulse_envelope(times: np.ndarray, n: int, sigma_s: float = 0.05) -> np.ndarray:
    env = np.zeros(n, dtype=np.float64)
    idx = np.round(times / GRID_S).astype(int)
    env[idx[(idx >= 0) & (idx < n)]] = 1.0
    radius = int(4 * sigma_s / GRID_S)
    t = np.arange(-radius, radius + 1) * GRID_S
    kernel = np.exp(-0.5 * (t / sigma_s) ** 2)
    return np.convolve(env, kernel, mode="same")


def znorm(x: np.ndarray) -> np.ndarray:
    s = x.std()
    return (x - x.mean()) / s if s > 0 else x * 0.0


def refine_offset(
    flac: Path, onsets: np.ndarray, center_s: float, half_window_s: float
) -> tuple[float, float]:
    """(offset_s, r_peak): sub-frame refinement of a DTW-coarse offset.

    The mel-DTW offset is quantized to its ~46 ms hop — too coarse next to
    the ±70 ms beat tolerance — so the 10 ms JAMS-onset grid does the last
    mile. In a narrow window around a whole-track-verified coarse offset the
    true peak dominates; r is recorded, not gated (absolute r of a sparse
    impulse train vs a dense envelope is low even on true matches — that is
    why the wide-window version of this could not be the gate)."""
    lag_lo_s = center_s - half_window_s
    lag_hi_s = center_s + half_window_s
    load_s = JAMS_SPAN_S + max(0.0, lag_hi_s) + 1.0
    y, sr = librosa.load(flac, sr=22050, mono=True, duration=load_s)
    hop = 512
    strength = librosa.onset.onset_strength(y=y, sr=sr, hop_length=hop)
    frame_t = librosa.frames_to_time(np.arange(len(strength)), sr=sr, hop_length=hop)
    n_ref = int(JAMS_SPAN_S / GRID_S)
    n_fetch = int(load_s / GRID_S)
    grid_t = np.arange(n_fetch) * GRID_S
    fetched = np.interp(grid_t, frame_t, strength)
    ref = znorm(impulse_envelope(onsets, n_ref))

    lags = np.arange(int(lag_lo_s / GRID_S), int(lag_hi_s / GRID_S) + 1)
    corr = np.full(len(lags), np.nan)
    for i, lag in enumerate(lags):
        a = fetched[max(lag, 0) : lag + n_ref]
        b = ref[max(-lag, 0) : max(-lag, 0) + len(a)]
        if len(a) > 500:
            corr[i] = float(np.dot(znorm(a), b[: len(a)]) / len(a))
    best = int(np.nanargmax(corr))
    return float(lags[best] * GRID_S), float(corr[best])


def dtw_align(flac: Path, mel_path: Path, info: dict) -> tuple[float, float, float]:
    """(coarse_offset_s, on_line_fraction, median_residual_ms).

    Subsequence DTW of the authors' original-audio melspec inside ours of the
    fetched video is both the offset ESTIMATOR (path median, ~46 ms frames)
    and the whole-span VERIFIER: a wrong video, a mid-track edit or a speed
    difference walks the path off a straight line and the on-line fraction
    collapses."""
    # info.json keys are uppercase (verified from the distributed archive:
    # SR 22050, N_MELS 80, N_FFT 2048, HOP_LENGTH 1024, librosa 0.7.0)
    sr = int(info.get("SR", 22050))
    hop = int(info.get("HOP_LENGTH", 1024))
    n_fft = int(info.get("N_FFT", 2048))
    n_mels = int(info.get("N_MELS", 80))

    ref_mel = load_melspec(mel_path)
    y, _ = librosa.load(flac, sr=sr, mono=True)
    our_mel = librosa.feature.melspectrogram(
        y=y, sr=sr, n_fft=n_fft, hop_length=hop, n_mels=ref_mel.shape[0] or n_mels
    )
    ref_db = librosa.power_to_db(np.maximum(ref_mel, 1e-10))
    our_db = librosa.power_to_db(np.maximum(our_mel, 1e-10))

    # Subsequence mode: the original (annotated) audio is expected to appear
    # as a contiguous span inside the fetched video.
    _, wp = librosa.sequence.dtw(X=ref_db, Y=our_db, metric="euclidean", subseq=True)
    wp = wp[::-1]  # start -> end
    ref_f, our_f = wp[:, 0].astype(np.float64), wp[:, 1].astype(np.float64)
    frame_s = hop / sr
    diff_s = (our_f - ref_f) * frame_s
    offset_s = float(np.median(diff_s))
    residual_ms = np.abs(diff_s - offset_s) * 1000.0
    on_line = float(np.mean(residual_ms <= 60.0))
    return offset_s, on_line, float(np.median(residual_ms))


def run(args) -> int:
    manifest = dl.load_manifest("harmonix")
    gate = manifest["gate"]
    dirs = dl.dataset_dirs("harmonix")
    status = dl.Status(dirs.status, "harmonix")
    repo = dirs.raw / fh.REPO_DIR

    class Ctx:  # the two fetch_harmonix helpers only need dirs
        pass

    ctx = Ctx()
    ctx.dirs = dirs
    tables = fh.load_tables(ctx)
    mel_dir = dirs.raw / "melspecs"
    info = load_mel_info(mel_dir)

    alignment = status.data.setdefault("alignment", {})
    ids = sorted(t for t, v in status.outcomes("fetch").items() if v == "ok")
    if args.only:
        ids = [i for i in ids if i in set(args.only)]
    if args.limit:
        ids = ids[: args.limit]

    def annotated_span(file_id: str) -> float:
        """The audio the annotations actually need — metadata Duration counts
        trailing silence the video may legitimately trim."""
        ds = repo / "dataset"
        beats, _ = fh.parse_beats(ds / "beats_and_downbeats" / f"{file_id}.txt")
        rows = fh.parse_segments(ds / "segments" / f"{file_id}.txt")
        return max(beats[-1] if beats else 0.0, rows[-1][0] if rows else 0.0)

    def gate_one(file_id: str, rec: dict) -> str:
        flac = dirs.raw / "full" / f"{file_id}.flac"
        duration = dl.ffprobe_duration(flac)
        expected = tables[file_id]["duration"]
        rec["duration_s"] = round(duration, 3)
        rec["expected_duration_s"] = expected
        if not expected:
            return "reject:no_expected_duration"
        span = annotated_span(file_id)
        rec["annotated_span_s"] = round(span, 3)
        tol = gate["shorter_tolerance_s"]
        if duration + tol < span:
            return f"reject:duration_short ({duration:.1f}s vs {span:.1f}s annotated)"

        mel_path = find_melspec(mel_dir, file_id)
        if mel_path is None:
            return "reject:no_melspec"
        coarse, on_line, residual_ms = dtw_align(flac, mel_path, info)
        rec["dtw_offset_s"] = round(coarse, 3)
        rec["dtw_on_line"] = round(on_line, 4)
        rec["dtw_residual_ms"] = round(residual_ms, 1)
        if residual_ms > gate["dtw"]["median_residual_ms"]:
            return f"reject:dtw_residual ({residual_ms:.0f}ms)"
        if on_line < gate["dtw"]["on_line_min"]:
            return f"reject:dtw_on_line ({on_line:.2f})"
        # A slightly negative offset = YouTube trimmed the song's head; the
        # missing seconds sit inside the standard 5 s beat trim, so prep can
        # rescue it by shifting annotations and dropping pre-roll events.
        if coarse < -gate["head_missing_max_s"]:
            return f"reject:offset_negative ({coarse:+.1f}s)"
        if coarse + span > duration + tol:
            return (
                f"reject:coverage (song at {coarse:+.1f}s needs {span:.1f}s, "
                f"video has {duration - coarse:.1f}s left)"
            )

        onsets = jams_onsets(repo, file_id)
        if len(onsets) < 5:
            return "reject:no_jams_onsets"
        offset, r_peak = refine_offset(
            flac, onsets, coarse, gate["refine"]["half_window_s"]
        )
        rec["offset_s"] = round(offset, 4)
        rec["xcorr_r"] = round(r_peak, 4)
        if abs(offset - coarse) > gate["refine"]["agreement_s"]:
            return f"reject:refine_disagrees (fine {offset:+.2f}s vs dtw {coarse:+.2f}s)"
        return "pass" if abs(offset) <= gate["offset_pass_s"] else "pass_with_offset"

    try:
        for i, file_id in enumerate(ids):
            if file_id in alignment and not args.force:
                continue
            rec: dict = {"upstream_score": tables[file_id]["upstream_score"]}
            try:
                rec["verdict"] = gate_one(file_id, rec)
                up = rec.get("upstream_score")
                if up is not None and up < gate["upstream_weak_below"]:
                    rec["flag"] = "upstream_weak"
            except Exception as e:
                rec["verdict"] = f"reject:error ({str(e).splitlines()[0][:120]})"
            alignment[file_id] = rec
            print(
                f"  {file_id}: {rec['verdict']}"
                + (f" off={rec.get('offset_s', rec.get('dtw_offset_s'))}s"
                   f" line={rec.get('dtw_on_line')} res={rec.get('dtw_residual_ms')}ms"
                   if "dtw_offset_s" in rec else "")
            )
            if i % 10 == 9:
                status.save()
    finally:
        status.save()
    return report(status)


def report(status) -> int:
    alignment = status.data.get("alignment", {})
    verdicts = {}
    for rec in alignment.values():
        key = rec.get("verdict", "?").split(" ")[0]
        verdicts[key] = verdicts.get(key, 0) + 1
    total = len(alignment)
    print(f"alignment: {total} gated — " + ", ".join(
        f"{k}: {v}" for k, v in sorted(verdicts.items())
    ))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run", help="gate all fetched tracks lacking a verdict")
    p.add_argument("--only", action="append")
    p.add_argument("--limit", type=int)
    p.add_argument("--force", action="store_true", help="re-gate existing verdicts")
    sub.add_parser("report", help="print verdict counts")
    args = ap.parse_args()
    if args.cmd == "report":
        dirs = dl.dataset_dirs("harmonix")
        return report(dl.Status(dirs.status, "harmonix"))
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
