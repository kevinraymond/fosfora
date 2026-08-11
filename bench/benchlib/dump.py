"""Parser for `--signal-dump` JSONL — the frozen contract this harness scores.

Line 1 is the meta record (`meta` key present); every other line is
`{"ts": <sample-clock seconds, hop end>, "addr": "/fosfora/v1/...",
"args": [{"i"|"f"|"s": value}, ...]}`. Event args are running counts, so a gap
is datagram loss on a live wire — in a dump it can only be a bug, and the
accessors treat it as a parse-level error, never a scoring outcome.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

PREFIX = "/fosfora/v1"

BEAT = f"{PREFIX}/beat"
DOWNBEAT = f"{PREFIX}/downbeat"
DROP = f"{PREFIX}/drop"
ONSET = f"{PREFIX}/onset"
BPM = f"{PREFIX}/bpm"
BAR_PHASE = f"{PREFIX}/bar_phase"
BUILD = f"{PREFIX}/build"
ENERGY = f"{PREFIX}/energy"
KEY = f"{PREFIX}/key"
SECTION = f"{PREFIX}/section"
SECTION_BOUNDARY = f"{PREFIX}/section/boundary"
PHRASE_LEN = f"{PREFIX}/phrase/len"
PREDICT_DROP = f"{PREFIX}/predict/drop"
STEM_ENERGY = {
    "drums": f"{PREFIX}/stem/drums/energy",
    "bass": f"{PREFIX}/stem/bass/energy",
    "melody": f"{PREFIX}/stem/melody/energy",
}

_ARG_TAGS = ("i", "f", "s")


class DumpError(ValueError):
    """The file violates the --signal-dump contract."""


def _decode_arg(arg: dict, lineno: int):
    if not isinstance(arg, dict) or len(arg) != 1:
        raise DumpError(f"line {lineno}: malformed arg {arg!r}")
    tag, value = next(iter(arg.items()))
    if tag not in _ARG_TAGS:
        raise DumpError(f"line {lineno}: unsupported arg tag {tag!r} ({value!r})")
    return value


class SignalDump:
    """One parsed dump: meta + the ordered (ts, addr, args) records."""

    def __init__(self, meta: dict, records: list[tuple[float, str, list]]):
        self.meta = meta
        self.records = records
        self.hop_hz: float = float(meta["hop_hz"])
        self.sample_rate: int = int(meta["sample_rate"])
        self.tx_rate_hz: int = int(meta["tx_rate_hz"])
        self.source: str = str(meta["source"])

    @classmethod
    def load(cls, path: str | Path) -> "SignalDump":
        path = Path(path)
        meta: dict | None = None
        records: list[tuple[float, str, list]] = []
        with path.open(encoding="utf-8") as f:
            for lineno, line in enumerate(f, start=1):
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except json.JSONDecodeError as e:
                    raise DumpError(f"line {lineno}: not JSON ({e})") from e
                if lineno == 1:
                    if obj.get("meta") != 1:
                        raise DumpError("line 1 is not the meta record")
                    if obj.get("schema") != PREFIX:
                        raise DumpError(f"unknown schema {obj.get('schema')!r}")
                    meta = obj
                    continue
                try:
                    ts, addr, args = obj["ts"], obj["addr"], obj["args"]
                except KeyError as e:
                    raise DumpError(f"line {lineno}: missing {e}") from e
                records.append(
                    (float(ts), addr, [_decode_arg(a, lineno) for a in args])
                )
        if meta is None:
            raise DumpError(f"{path}: empty file")
        return cls(meta, records)

    def events(self, addr: str) -> list[tuple[float, list]]:
        """All (ts, args) records for one address, in dump order."""
        return [(ts, args) for ts, a, args in self.records if a == addr]

    def counted_events(self, addr: str) -> np.ndarray:
        """Timestamps of an event address whose arg is the running count.

        Verifies the counts are exactly 1..N — the wire's zero-loss property.
        """
        events = self.events(addr)
        counts = [args[0] for _, args in events]
        if counts != list(range(1, len(counts) + 1)):
            raise DumpError(f"{addr}: running counts are not 1..N: {counts[:10]}...")
        return np.array([ts for ts, _ in events], dtype=np.float64)

    def beats(self) -> np.ndarray:
        return self.counted_events(BEAT)

    def downbeats(self) -> np.ndarray:
        return self.counted_events(DOWNBEAT)

    def drops(self) -> np.ndarray:
        return self.counted_events(DROP)

    def series(self, addr: str) -> tuple[np.ndarray, np.ndarray]:
        """(ts, value) arrays for a continuous float address."""
        events = self.events(addr)
        ts = np.array([t for t, _ in events], dtype=np.float64)
        vals = np.array([args[0] for _, args in events], dtype=np.float64)
        return ts, vals

    def changes(self, addr: str, key=None) -> list[tuple[float, list]]:
        """On-change records with the 1 Hz re-broadcasts stripped.

        Keeps record i iff key(args_i) != key(args_{i-1}). The default key is
        the first arg — deliberately not the confidence, which re-broadcasts
        may legitimately update for the same state.
        """
        key = key or (lambda args: args[0])
        out: list[tuple[float, list]] = []
        prev = object()
        for ts, args in self.events(addr):
            k = key(args)
            if k != prev:
                out.append((ts, args))
                prev = k
        return out
