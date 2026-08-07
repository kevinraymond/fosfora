#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "mir_eval>=0.7", "soundfile"]
# ///
"""Unit tests for benchlib: the dump parser, annotation bundles, cache keys,
results plumbing and (as they land) every hand-rolled metric rule. Plain
stdlib unittest, run as:

    bench/test_benchlib.py
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

from benchlib import results
from benchlib.annotations import Annotations, key_to_mir_eval, load_index
from benchlib.dump import DumpError, SignalDump
from benchlib.metrics import beats as m_beats
from benchlib.metrics import key as m_key
from benchlib.metrics import tempo as m_tempo
from benchlib.runner import flags_digest, dump_args

META = (
    '{"hop_hz":86.13,"meta":1,"sample_rate":44100,"schema":"/fosfora/v1",'
    '"source":"t.wav","tx_rate_hz":30}'
)


def write_dump(lines: list[str]) -> Path:
    f = tempfile.NamedTemporaryFile(
        "w", suffix=".jsonl", delete=False, encoding="utf-8"
    )
    f.write("\n".join(lines) + "\n")
    f.close()
    return Path(f.name)


def rec(ts: float, addr: str, args: list) -> str:
    return json.dumps({"ts": ts, "addr": addr, "args": args}, sort_keys=True)


def counted(addr: str, times) -> list[str]:
    """Event lines with the running-count arg the wire requires."""
    return [rec(float(t), addr, [{"i": i + 1}]) for i, t in enumerate(times)]


def series(addr: str, pairs) -> list[str]:
    return [rec(float(t), addr, [{"f": float(v)}]) for t, v in pairs]


def make_ann(**fields) -> Annotations:
    raw = {
        "schema": "fosfora-bench-annotation/v1",
        "dataset": "test",
        "track_id": "t1",
        "audio": {"path": "t1.wav", "duration_s": fields.pop("duration_s", 60.0)},
        **fields,
    }
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "t1.json"
        p.write_text(json.dumps(raw), encoding="utf-8")
        return Annotations.load(p)


class TestDumpParser(unittest.TestCase):
    def test_meta_and_events(self):
        d = SignalDump.load(
            write_dump(
                [
                    META,
                    rec(0.5, "/fosfora/v1/beat", [{"i": 1}]),
                    rec(1.0, "/fosfora/v1/beat", [{"i": 2}]),
                    rec(1.0, "/fosfora/v1/downbeat", [{"i": 1}]),
                ]
            )
        )
        self.assertEqual(d.sample_rate, 44100)
        self.assertAlmostEqual(d.hop_hz, 86.13)
        np.testing.assert_allclose(d.beats(), [0.5, 1.0])
        np.testing.assert_allclose(d.downbeats(), [1.0])
        np.testing.assert_allclose(d.drops(), [])

    def test_missing_meta_rejected(self):
        with self.assertRaises(DumpError):
            SignalDump.load(write_dump([rec(0.5, "/fosfora/v1/beat", [{"i": 1}])]))

    def test_count_gap_is_a_parse_error(self):
        d = SignalDump.load(
            write_dump(
                [
                    META,
                    rec(0.5, "/fosfora/v1/beat", [{"i": 1}]),
                    rec(1.5, "/fosfora/v1/beat", [{"i": 3}]),  # 2 lost
                ]
            )
        )
        with self.assertRaises(DumpError):
            d.beats()

    def test_unsupported_arg_tag_rejected(self):
        with self.assertRaises(DumpError):
            SignalDump.load(
                write_dump([META, rec(0.5, "/fosfora/v1/beat", [{"blob": "x"}])])
            )

    def test_series(self):
        d = SignalDump.load(
            write_dump(
                [
                    META,
                    rec(0.1, "/fosfora/v1/bpm", [{"f": 120.0}]),
                    rec(0.2, "/fosfora/v1/bpm", [{"f": 121.0}]),
                ]
            )
        )
        ts, vals = d.series("/fosfora/v1/bpm")
        np.testing.assert_allclose(ts, [0.1, 0.2])
        np.testing.assert_allclose(vals, [120.0, 121.0])

    def test_changes_strips_rebroadcasts_but_not_confidence_updates(self):
        d = SignalDump.load(
            write_dump(
                [
                    META,
                    rec(1.0, "/fosfora/v1/section", [{"s": "intro"}, {"f": 0.5}]),
                    # 1 Hz re-broadcast, same label, updated confidence: dropped
                    rec(2.0, "/fosfora/v1/section", [{"s": "intro"}, {"f": 0.6}]),
                    rec(3.0, "/fosfora/v1/section", [{"s": "build"}, {"f": 0.7}]),
                    rec(4.0, "/fosfora/v1/section", [{"s": "build"}, {"f": 0.7}]),
                    # back to a previously-seen label: kept (only *consecutive* dedupe)
                    rec(5.0, "/fosfora/v1/section", [{"s": "intro"}, {"f": 0.3}]),
                ]
            )
        )
        changes = d.changes("/fosfora/v1/section")
        self.assertEqual(
            [(ts, args[0]) for ts, args in changes],
            [(1.0, "intro"), (3.0, "build"), (5.0, "intro")],
        )


class TestAnnotations(unittest.TestCase):
    def bundle(self, **overrides) -> dict:
        raw = {
            "schema": "fosfora-bench-annotation/v1",
            "dataset": "test",
            "track_id": "t1",
            "audio": {"path": "t1.wav", "sr": 44100, "duration_s": 10.0},
            "beats": [0.5, 1.0, 1.5],
            "downbeats": [0.5],
            "tempo_bpm": 120.0,
            "key": {"tonic": "A", "mode": "minor", "raw": "a"},
            "segments": [[0.0, 5.0, "intro"], [5.0, 10.0, "chorus"]],
            "drops": [{"time": 5.0, "source": "test", "kind": "direct"}],
        }
        raw.update(overrides)
        return raw

    def load(self, raw: dict) -> Annotations:
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "t1.json"
            p.write_text(json.dumps(raw), encoding="utf-8")
            return Annotations.load(p)

    def test_full_bundle(self):
        ann = self.load(self.bundle())
        self.assertEqual(ann.key, "A minor")
        self.assertEqual(ann.duration_s, 10.0)
        np.testing.assert_allclose(ann.beats, [0.5, 1.0, 1.5])
        self.assertEqual(ann.segments[1], (5.0, 10.0, "chorus"))
        np.testing.assert_allclose(ann.drop_times(), [5.0])
        np.testing.assert_allclose(ann.drop_times(kinds={"proxy_chorus_onset"}), [])

    def test_absent_fields_are_none(self):
        ann = self.load(
            self.bundle(beats=None, key=None, segments=None, drops=None, tempo_bpm=None)
        )
        self.assertIsNone(ann.beats)
        self.assertIsNone(ann.key)
        self.assertIsNone(ann.segments)
        np.testing.assert_allclose(ann.drop_times(), [])

    def test_segments_require_duration(self):
        raw = self.bundle()
        raw["audio"] = {"path": "t1.wav"}
        with self.assertRaises(Exception):
            self.load(raw)

    def test_unsorted_beats_rejected(self):
        with self.assertRaises(Exception):
            self.load(self.bundle(beats=[1.0, 0.5]))

    def test_key_forms(self):
        self.assertEqual(key_to_mir_eval("Am"), "A minor")
        self.assertEqual(key_to_mir_eval("F#"), "F# major")
        self.assertEqual(key_to_mir_eval("A minor"), "A minor")
        self.assertEqual(
            key_to_mir_eval({"tonic": "C", "mode": "major"}), "C major"
        )
        self.assertIsNone(key_to_mir_eval(None))

    def test_index_roundtrip(self):
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            (td / "index.json").write_text(
                json.dumps(
                    {
                        "schema": "fosfora-bench-index/v1",
                        "dataset": "test",
                        "tracks": [
                            {"track_id": "t1", "audio": "t1.wav", "annotations": "t1.json"}
                        ],
                    }
                ),
                encoding="utf-8",
            )
            tracks = load_index(td / "index.json")
            self.assertEqual(tracks[0]["track_id"], "t1")
            self.assertEqual(tracks[0]["audio"], (td / "t1.wav").resolve())


class TestRunner(unittest.TestCase):
    def test_flags_digest(self):
        self.assertEqual(flags_digest(None, False, False), "r30_fb0_st1")
        self.assertEqual(flags_digest(10, True, True), "r10_fb1_st0")

    def test_dump_args(self):
        self.assertEqual(dump_args(None, False, False), [])
        self.assertEqual(
            dump_args(10, True, True), ["--rate", "10", "--feat-bus", "--no-stems"]
        )


class TestResults(unittest.TestCase):
    def test_round_floats(self):
        rounded = results.round_floats(
            {"a": 0.123456789, "b": [1.23456], "c": {"d": float("nan")}}
        )
        self.assertEqual(rounded["a"], 0.1235)
        self.assertEqual(rounded["b"], [1.2346])
        self.assertIsNone(rounded["c"]["d"])

    def test_generic_aggregate(self):
        agg = results.generic_aggregate(
            [
                {"f": 0.8, "hit": True, "nested": {"x": 1.0}},
                {"f": 0.6, "hit": False},
            ]
        )
        self.assertAlmostEqual(agg["f"]["mean"], 0.7)
        self.assertEqual(agg["f"]["n"], 2)
        self.assertAlmostEqual(agg["hit"]["mean"], 0.5)
        self.assertEqual(agg["nested.x"]["n"], 1)

    def test_aggregate_shape(self):
        rs = [
            {"dataset": "d", "metrics": {"beat": {"f_measure": 0.9}}},
            {"dataset": "d", "metrics": {"beat": {"f_measure": 0.7}, "key": None}},
        ]
        summary = results.aggregate(rs)
        self.assertEqual(summary["n_tracks"], 2)
        self.assertEqual(summary["metrics"]["beat"]["n_tracks"], 2)
        self.assertAlmostEqual(summary["metrics"]["beat"]["f_measure"]["mean"], 0.8)
        self.assertNotIn("key", summary["metrics"])


class TestBeatMetric(unittest.TestCase):
    def test_identical_beats_score_one(self):
        times = np.arange(0.5, 40.0, 0.5)
        dump = SignalDump.load(write_dump([META, *counted("/fosfora/v1/beat", times)]))
        ann = make_ann(beats=list(times))
        block = m_beats.beat(dump, ann)
        self.assertAlmostEqual(block["f_measure"], 1.0)
        self.assertAlmostEqual(block["cmlt"], 1.0)
        self.assertAlmostEqual(block["amlt"], 1.0)
        self.assertAlmostEqual(block["f_measure_untrimmed"], 1.0)

    def test_offset_beyond_window_scores_zero(self):
        ref = np.arange(0.5, 40.0, 0.5)
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/beat", ref + 0.1)])
        )
        block = m_beats.beat(dump, make_ann(beats=list(ref)))
        self.assertAlmostEqual(block["f_measure"], 0.0)

    def test_offset_within_window_still_hits(self):
        ref = np.arange(0.5, 40.0, 0.5)
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/beat", ref + 0.03)])
        )
        block = m_beats.beat(dump, make_ann(beats=list(ref)))
        self.assertAlmostEqual(block["f_measure"], 1.0)

    def test_trim_hides_coldstart_untrimmed_does_not(self):
        ref = np.arange(0.5, 40.0, 0.5)
        est = ref[ref >= 6.0]  # detector silent for the first 6 s
        dump = SignalDump.load(write_dump([META, *counted("/fosfora/v1/beat", est)]))
        block = m_beats.beat(dump, make_ann(beats=list(ref)))
        self.assertGreater(block["f_measure"], 0.98)  # 5 s trim forgives most
        self.assertLess(block["f_measure_untrimmed"], block["f_measure"])


class TestTempoMetric(unittest.TestCase):
    def dump_with_bpm(self, pairs):
        return SignalDump.load(write_dump([META, *series("/fosfora/v1/bpm", pairs)]))

    def test_converged_estimate_and_lock_time(self):
        pairs = [(t, 100.0) for t in np.arange(0.0, 5.0, 0.5)] + [
            (t, 120.0) for t in np.arange(5.0, 10.0, 0.5)
        ]
        block = m_tempo.tempo(self.dump_with_bpm(pairs), make_ann(tempo_bpm=120.0))
        self.assertAlmostEqual(block["bpm_estimate"], 120.0)
        self.assertTrue(block["acc1"])
        self.assertAlmostEqual(block["lock_time_secs"], 5.0)
        self.assertAlmostEqual(block["locked_fraction"], 0.5)

    def test_octave_error_fails_acc1_passes_acc2(self):
        pairs = [(t, 120.0) for t in np.arange(0.0, 10.0, 0.5)]
        block = m_tempo.tempo(self.dump_with_bpm(pairs), make_ann(tempo_bpm=60.0))
        self.assertFalse(block["acc1"])
        self.assertTrue(block["acc2"])
        self.assertIsNone(block["lock_time_secs"])
        self.assertAlmostEqual(block["lock_time_acc2_secs"], 0.0)

    def test_zero_samples_is_no_estimate(self):
        block = m_tempo.tempo(
            self.dump_with_bpm([(1.0, 0.0)]), make_ann(tempo_bpm=120.0)
        )
        self.assertTrue(block["no_estimate"])


class TestKeyMetric(unittest.TestCase):
    def test_duration_weighted_majority(self):
        lines = [
            META,
            rec(1.0, "/fosfora/v1/key", [{"s": "Am"}, {"f": 0.4}]),
            rec(3.0, "/fosfora/v1/key", [{"s": "C"}, {"f": 0.8}]),
            # 1 Hz re-broadcast of the same key must not double-count duration
            rec(4.0, "/fosfora/v1/key", [{"s": "C"}, {"f": 0.8}]),
            rec(30.0, "/fosfora/v1/bpm", [{"f": 120.0}]),  # extends track end
        ]
        block = m_key.key(SignalDump.load(write_dump(lines)), make_ann(key="C major"))
        self.assertEqual(block["estimated_key"], "C major")  # held 27 s vs Am 2 s
        self.assertAlmostEqual(block["score"], 1.0)
        self.assertAlmostEqual(block["first_emit_ts"], 1.0)
        self.assertEqual(block["n_changes"], 2)

    def test_relative_minor_scores_mirex_weight(self):
        lines = [META, rec(1.0, "/fosfora/v1/key", [{"s": "Am"}, {"f": 0.5}])]
        block = m_key.key(SignalDump.load(write_dump(lines)), make_ann(key="C major"))
        self.assertAlmostEqual(block["score"], 0.3)

    def test_silent_detector_is_no_estimate(self):
        block = m_key.key(SignalDump.load(write_dump([META])), make_ann(key="C major"))
        self.assertTrue(block["no_estimate"])

    def test_aggregator_counts_silence_as_zero(self):
        agg = m_key._aggregate(
            [
                {"score": 1.0, "first_emit_ts": 2.0},
                {"no_estimate": True},
            ]
        )
        self.assertAlmostEqual(agg["score"]["mean"], 0.5)
        self.assertAlmostEqual(agg["no_estimate_rate"], 0.5)


if __name__ == "__main__":
    unittest.main()
