#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""The reader against a dump the Rust writer produced.

fixtures/click_50hops.jsonl is the header and first 50 hops of `fosfora --signal-dump` on
the synthetic click fixtures/make_click_fixture.py describes (0.5 s of silence, then a
120 BPM kick), with FOSFORA_HOP_SIDECAR set. Runs as a script or under pytest.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import hopdump  # noqa: E402

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "click_50hops.jsonl"
H = 50


def load() -> hopdump.HopDump:
    return hopdump.read(FIXTURE)


def test_header_and_shapes():
    d = load()
    assert d.sr == 44100 and d.hop == 512
    assert abs(d.hop_hz - 86.1328125) < 1e-9
    assert d.n_hops == H
    assert d.kf.shape == (H, 24) and d.sf.shape == (H, 4) and d.pn.shape == (H, 85)
    assert d.kick_prob.shape == (H,) and d.silent.shape == (H,)
    assert d.version == 2 and d.act.shape == (H,) and np.isfinite(d.act).all() and (0 <= d.act).all() and (d.act <= 1).all()
    assert list(d.idx) == list(range(H))
    assert np.allclose(d.ts, (d.idx + 1) * d.hop / d.sr, atol=1e-6)
    assert d.kf_names[0] == "flux_30_60" and d.kf_names[11] == "level_30_60"
    assert d.kf_names[-2:] == ["low_flatness", "low_peakiness"]
    assert d.sf_names == ["combined", "low", "mid", "high"]
    assert d.pn_names[0] == "sub_bass" and d.pn_names[8] == "kick" and d.pn_names[-1] == "beat_locked"
    assert np.isfinite(d.kf).all() and np.isfinite(d.sf).all() and np.isfinite(d.pn).all()


def test_the_silent_lead_in_is_gated():
    d = load()
    # 0.5 s of silence is the first ~43 hops; the loudness gate opens a little after the
    # first click, so the fixture has both silent and sounding hops.
    assert d.silent[:20].all(), "the lead-in did not read as silent"
    assert not d.silent.all(), "no hop ever read as sounding"
    # Under silence `kick` is gated to 0 and the onset flux is zero, while the kick model's
    # raw probability is still scored every hop (the history must stay aligned).
    assert (d.column("kick")[d.silent] == 0).all()
    assert (d.sf[d.silent] == 0).all()
    assert (d.sf[~d.silent, 0] > 0).any(), "the onset detector never saw the click"
    assert len(d.beat_times) == 0, "no grid can fire a beat inside 0.6 s"


def test_acoustic_drops_the_trackers_slots():
    d = load()
    x, names = d.acoustic()
    assert x.shape == (H, 85 - len(hopdump.NOT_ACOUSTIC))
    assert len(hopdump.NOT_ACOUSTIC) == 18
    assert not (set(names) & hopdump.NOT_ACOUSTIC)
    assert "kick" in names and "rms" in names and "mfcc.0" in names and "timbre_flux" in names
    assert "onset" not in names and "beat" not in names and "bpm" not in names


def test_stack_context_matches_the_runtime_order():
    d = load()
    s = hopdump.stack_context(d.kf, 2)
    assert s.shape == (H, 72)
    assert (s[5, :24] == d.kf[5]).all()
    assert (s[5, 24:48] == d.kf[4]).all()
    assert (s[5, 48:] == d.kf[3]).all()
    assert (s[0, 24:] == 0).all() and (s[1, 48:] == 0).all()
    assert (hopdump.stack_context(d.kf, 0) == d.kf).all()


def test_pinned_values():
    """Literal numbers from the fixture: a column shuffle or a formatting change in the
    writer fails here, not in a model trained on the wrong column."""
    d = load()
    # hop 0: inside the silent lead-in
    assert d.silent[0] and d.ts[0] == 0.01161
    assert d.kick_prob[0] == np.float32(0.31266862)
    assert (d.sf[0] == 0).all() and d.column("kick")[0] == 0
    assert d.kf[0, 0] == np.float32(0) and d.kf[0, 11] == np.float32(0)
    assert d.column("rms")[0] == np.float32(0)
    # hop 44: the first click has landed
    assert not d.silent[44] and d.ts[44] == 0.522449
    assert d.kick_prob[44] == np.float32(0.9473521)
    assert d.kf[44, 0] == np.float32(1.0342047)     # flux_30_60
    assert d.kf[44, 23] == np.float32(1.8965757)   # low_peakiness
    assert d.sf[44].tolist() == [np.float32(v) for v in [1.1524177, 1.1291741, 1.1738008, 1.2187415]]
    assert d.column("sub_bass")[44] == np.float32(0.28550395)
    assert d.column("kick")[44] == np.float32(0.8683803)
    assert d.column("mfcc.0")[44] == np.float32(-113.440216)


def main() -> int:
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    failed = 0
    for t in tests:
        try:
            t()
            print(f"ok    {t.__name__}")
        except Exception as e:  # noqa: BLE001
            failed += 1
            print(f"FAIL  {t.__name__}: {type(e).__name__}: {e}")
    print(f"{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
