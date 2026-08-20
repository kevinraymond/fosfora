"""Normalized annotation bundles — `fosfora-bench-annotation/v1`.

One JSON per track, produced by the per-dataset converters (fetch_*.py `prep`)
and by the fixture generator. All time fields are seconds on the local audio
file's clock (offset corrections already baked in — the engine has no
resampler). Fields are null/absent when the dataset doesn't carry that signal;
the metric registry keys off field presence.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

SCHEMA = "fosfora-bench-annotation/v1"
INDEX_SCHEMA = "fosfora-bench-index/v1"


class AnnotationError(ValueError):
    """The bundle violates the fosfora-bench-annotation/v1 schema."""


def _times(raw, name: str) -> np.ndarray | None:
    if raw is None:
        return None
    arr = np.asarray(raw, dtype=np.float64)
    if arr.ndim != 1:
        raise AnnotationError(f"{name}: expected a flat list of seconds")
    if len(arr) > 1 and np.any(np.diff(arr) < 0):
        raise AnnotationError(f"{name}: times must be non-decreasing")
    return arr


def key_to_mir_eval(key) -> str | None:
    """Normalize the bundle's key field to mir_eval's '<tonic> <mode>' form.

    Accepts the canonical {tonic, mode} object, or a plain string in either
    mir_eval form ('A minor') or the wire form ('Am' / 'F#').
    """
    if key is None:
        return None
    if isinstance(key, dict):
        tonic, mode = key.get("tonic"), key.get("mode")
        if not tonic or mode not in ("major", "minor"):
            raise AnnotationError(f"key: bad object {key!r}")
        return f"{tonic} {mode}"
    if isinstance(key, str):
        s = key.strip()
        if " " in s:
            return s
        if s.endswith("m"):
            return f"{s[:-1]} minor"
        return f"{s} major"
    raise AnnotationError(f"key: unsupported value {key!r}")


class Annotations:
    """Typed view over one bundle. Absent signal -> attribute is None."""

    def __init__(self, raw: dict, base_dir: Path):
        if raw.get("schema") != SCHEMA:
            raise AnnotationError(f"schema is {raw.get('schema')!r}, want {SCHEMA!r}")
        self.raw = raw
        self.base_dir = Path(base_dir)
        self.dataset: str = raw.get("dataset", "?")
        self.track_id: str = raw["track_id"]

        audio = raw.get("audio") or {}
        self.audio_path: Path | None = (
            (base_dir / audio["path"]).resolve() if audio.get("path") else None
        )
        self.duration_s: float | None = (
            float(audio["duration_s"]) if audio.get("duration_s") is not None else None
        )

        self.beats = _times(raw.get("beats"), "beats")
        self.downbeats = _times(raw.get("downbeats"), "downbeats")
        self.tempo_bpm: float | None = (
            float(raw["tempo_bpm"]) if raw.get("tempo_bpm") is not None else None
        )
        self.key: str | None = key_to_mir_eval(raw.get("key"))

        segs = raw.get("segments")
        self.segments: list[tuple[float, float, str]] | None = (
            [(float(s), float(e), str(label)) for s, e, label in segs]
            if segs is not None
            else None
        )
        if self.segments is not None and self.duration_s is None:
            raise AnnotationError("segments present but audio.duration_s missing")

        self.drops: list[dict] | None = raw.get("drops")
        # Additive since the labelling pass (#2299): instants the listener was
        # shown and ruled *not* a drop. Absent on every dataset-derived bundle,
        # so nothing outside bench/labels/ changes behavior.
        self.not_drops: list[dict] | None = raw.get("not_drops")
        self.stems: dict | None = raw.get("stems")

    @classmethod
    def load(cls, path: str | Path) -> "Annotations":
        path = Path(path)
        with path.open(encoding="utf-8") as f:
            raw = json.load(f)
        return cls(raw, path.parent)

    def drop_times(self, kinds: set[str] | None = None) -> np.ndarray:
        """Annotated drop instants, optionally filtered by derivation kind
        ('direct' | 'proxy_chorus_onset' | 'local_manual' | 'constructed')."""
        if not self.drops:
            return np.array([], dtype=np.float64)
        times = [
            float(d["time"])
            for d in self.drops
            if kinds is None or d.get("kind") in kinds
        ]
        return np.array(sorted(times), dtype=np.float64)

    def not_drop_times(self) -> np.ndarray:
        """Instants explicitly ruled not-a-drop. Empty when the bundle carries
        none — which is every dataset-derived bundle, by construction."""
        if not self.not_drops:
            return np.array([], dtype=np.float64)
        times = [float(d["time"]) for d in self.not_drops]
        return np.array(sorted(times), dtype=np.float64)


def load_index(path: str | Path) -> list[dict]:
    """`norm/index.json` -> [{track_id, audio, annotations}] with paths resolved."""
    path = Path(path)
    with path.open(encoding="utf-8") as f:
        raw = json.load(f)
    if raw.get("schema") != INDEX_SCHEMA:
        raise AnnotationError(f"index schema is {raw.get('schema')!r}")
    base = path.parent
    out = []
    for t in raw["tracks"]:
        out.append(
            {
                "track_id": t["track_id"],
                "audio": (base / t["audio"]).resolve(),
                "annotations": (base / t["annotations"]).resolve(),
            }
        )
    return out
