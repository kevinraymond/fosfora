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

import contextlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

import datasetlib as dl
from benchlib import metrics as m_metrics
from benchlib import results
from benchlib.annotations import Annotations, key_to_mir_eval, load_index
from benchlib.dump import DumpError, SignalDump
from benchlib.metrics import beats as m_beats
from benchlib.metrics import drops as m_drops
from benchlib.metrics import key as m_key
from benchlib.metrics import stems as m_stems
from benchlib.metrics import structure as m_structure
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

    def test_aggregator_taxonomy_and_mode(self):
        def block(score, est, ref):
            return {"score": score, "estimated_key": est, "ref_key": ref,
                    "first_emit_ts": 1.0}

        agg = m_key._aggregate(
            [
                block(1.0, "C major", "C major"),
                block(0.5, "G major", "C major"),
                block(0.3, "A minor", "C major"),
                block(0.2, "C minor", "C major"),
                block(0.0, "B minor", "C major"),
                # Silence is a miss in every column, incl. major recall.
                {"no_estimate": True, "ref_key": "D major"},
            ]
        )
        self.assertEqual(
            agg["taxonomy"],
            {"exact": 1, "fifth": 1, "relative": 1, "parallel": 1,
             "other": 1, "none": 1},
        )
        self.assertAlmostEqual(agg["mode_accuracy"], 2 / 6)
        self.assertAlmostEqual(agg["major_mode_recall"], 2 / 6)


def section_lines(changes: list[tuple[float, str]]) -> list[str]:
    return [
        rec(ts, "/fosfora/v1/section", [{"s": label}, {"f": 0.7}])
        for ts, label in changes
    ]


class TestStructureMetric(unittest.TestCase):
    REF = {"segments": [[0.0, 10.0, "A"], [10.0, 20.0, "B"], [20.0, 30.0, "C"]],
           "duration_s": 30.0}

    def test_matching_partition_different_vocabulary(self):
        dump = SignalDump.load(
            write_dump(
                [META, *section_lines([(0.0116, "intro"), (10.0, "build"), (20.0, "steady")])]
            )
        )
        block = m_structure.structure(dump, make_ann(**self.REF))
        self.assertAlmostEqual(block["boundary_0_5s"]["f"], 1.0)
        self.assertAlmostEqual(block["pairwise"]["f"], 1.0)

    def test_lag_compensation_is_quarantined(self):
        dump = SignalDump.load(
            write_dump(
                [META, *section_lines([(0.0116, "intro"), (13.0, "build"), (23.0, "steady")])]
            )
        )
        block = m_structure.structure(dump, make_ann(**self.REF))
        self.assertAlmostEqual(block["boundary_0_5s"]["f"], 0.0)
        self.assertAlmostEqual(block["lag_compensated"]["boundary_0_5s"]["f"], 1.0)
        self.assertEqual(block["lag_compensated"]["lag_secs"], 3.0)

    def test_no_section_emissions(self):
        block = m_structure.structure(
            SignalDump.load(write_dump([META])), make_ann(**self.REF)
        )
        self.assertTrue(block["no_estimate"])


def drop_ann(
    times: list[float],
    duration_s: float = 60.0,
    not_drops: list[float] | None = None,
) -> Annotations:
    """Bar = 2 s, beat = 0.5 s — so 0.4 s is inside the beat grace and 1.7 s is
    inside the +-1 bar window but well outside it."""
    extra = (
        {"not_drops": [{"time": t, "kind": "local_manual"} for t in not_drops]}
        if not_drops is not None
        else {}
    )
    return make_ann(
        duration_s=duration_s,
        downbeats=list(np.arange(0.0, duration_s, 2.0)),
        beats=list(np.arange(0.0, duration_s, 0.5)),
        drops=[{"time": t, "source": "test", "kind": "direct"} for t in times],
        **extra,
    )


@contextlib.contextmanager
def negative_policy(name: str):
    conv = m_metrics.CONVENTIONS["drop"]
    prev = conv["negative_policy"]
    conv["negative_policy"] = name
    try:
        yield
    finally:
        conv["negative_policy"] = prev


class TestDropMetric(unittest.TestCase):
    def test_hit_within_one_bar(self):
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/drop", [20.5])])
        )
        block = m_drops.drop(dump, drop_ann([20.0]))
        self.assertAlmostEqual(block["hit_rate"], 1.0)
        self.assertEqual(block["n_false"], 0)

    def test_miss_beyond_one_bar_and_false_drop(self):
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/drop", [23.0, 40.0])])
        )
        block = m_drops.drop(dump, drop_ann([20.0]))  # bar = 2 s, |23-20| > 2
        self.assertAlmostEqual(block["hit_rate"], 0.0)
        self.assertEqual(block["n_false"], 2)
        self.assertAlmostEqual(block["false_drops_per_min"], 2.0)

    def test_greedy_matching_is_one_to_one(self):
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/drop", [19.9, 20.1])])
        )
        block = m_drops.drop(dump, drop_ann([20.0]))
        self.assertEqual(block["n_matched"], 1)
        self.assertEqual(block["n_false"], 1)

    def test_zero_drop_track_feeds_false_rate(self):
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/drop", [10.0])])
        )
        block = m_drops.drop(dump, drop_ann([]))
        self.assertIsNone(block["hit_rate"])
        self.assertEqual(block["n_false"], 1)

    # --- recorded negatives (#2299) -------------------------------------------
    # The three real cases these encode: Thirty Two Hertz 74.68 and 149.66 fired
    # 0.79 and 0.90 beat before a drop and were rejected as "buildup"; Thermal
    # Break 104.22 fired 3.97 beats early and was rejected as "break".

    def rejected_fire(self, fire: float, drop: float = 20.0):
        dump = SignalDump.load(write_dump([META, *counted("/fosfora/v1/drop", [fire])]))
        return dump, drop_ann([drop], not_drops=[fire])

    def test_rejected_fire_under_a_beat_early_still_hits(self):
        dump, ann = self.rejected_fire(19.6)  # 0.4 s = 0.8 beat
        with negative_policy("beat_grace"):
            block = m_drops.drop(dump, ann)
        self.assertEqual(block["n_matched"], 1)
        self.assertEqual(block["n_rejected"], 1)
        self.assertEqual(block["n_rejected_matched"], 1)

    def test_rejected_fire_a_bar_early_is_false_despite_the_window(self):
        dump, ann = self.rejected_fire(18.3)  # 1.7 s: inside +-1 bar, 3.4 beats
        with negative_policy("beat_grace"):
            block = m_drops.drop(dump, ann)
        self.assertEqual(block["n_matched"], 0)
        self.assertEqual(block["n_false"], 1)
        # ...and the pre-2026-08-20 convention scored exactly this as a hit.
        with negative_policy("bar_window"):
            self.assertEqual(m_drops.drop(dump, ann)["n_matched"], 1)

    def test_strict_policy_rejects_even_a_sub_beat_fire(self):
        dump, ann = self.rejected_fire(19.6)
        with negative_policy("strict"):
            block = m_drops.drop(dump, ann)
        self.assertEqual(block["n_matched"], 0)
        self.assertEqual(block["n_rejected_matched"], 0)

    def test_demoted_pair_frees_its_reference_for_another_fire(self):
        # Closest fire is rejected and 2.8 beats early; a second, unjudged fire
        # is farther but valid — the reference must go to it rather than be
        # eaten by the pair the policy forbids.
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/drop", [18.6, 21.5])])
        )
        ann = drop_ann([20.0], not_drops=[18.6])
        with negative_policy("beat_grace"):
            block = m_drops.drop(dump, ann)
        self.assertEqual(block["n_matched"], 1)
        self.assertEqual(block["n_rejected_matched"], 0)

    def test_bundle_without_negatives_is_policy_invariant(self):
        """The guard on every dataset-derived corpus: no `not_drops`, no change.

        Harmonix carries none on all 374 tracks, so a policy change must not be
        able to move a single one of its numbers."""
        dump = SignalDump.load(
            write_dump([META, *counted("/fosfora/v1/drop", [19.6, 18.3, 40.0])])
        )
        ann = drop_ann([20.0])
        blocks = []
        for name in m_metrics.NEGATIVE_POLICIES:
            with negative_policy(name):
                b = m_drops.drop(dump, ann)
                b.pop("negative_policy")
                blocks.append(b)
        self.assertEqual(blocks[0], blocks[1])
        self.assertEqual(blocks[0], blocks[2])

    def test_unknown_policy_is_fatal_not_a_silent_default(self):
        dump, ann = self.rejected_fire(19.6)
        with negative_policy("beat_grazie"), self.assertRaises(ValueError):
            m_drops.drop(dump, ann)

    def test_pooled_aggregation(self):
        agg = m_drops._aggregate_drop(
            [
                {"n_ref": 1, "n_est": 1, "n_matched": 1, "n_false": 0, "duration_min": 1.0},
                {"n_ref": 3, "n_est": 2, "n_matched": 1, "n_false": 1, "duration_min": 1.0},
            ]
        )
        self.assertAlmostEqual(agg["hit_rate"], 0.5)  # 2/4 pooled, not (1+1/3)/2
        self.assertAlmostEqual(agg["false_drops_per_min"], 0.5)


def predict_series(pairs) -> list[str]:
    return series("/fosfora/v1/predict/drop", pairs)


def grid(t0: float, t1: float, value, hz: float = 30.0):
    ts = np.arange(t0, t1, 1.0 / hz)
    fn = value if callable(value) else (lambda _t: value)
    return [(t, fn(t)) for t in ts]


class TestPredictDrop(unittest.TestCase):
    def test_sustained_crossing_and_lead_time(self):
        pairs = grid(0.0, 14.0, 0.0) + grid(14.0, 20.0, 0.9)
        dump = SignalDump.load(write_dump([META, *predict_series(pairs)]))
        block = m_drops.predict_drop(dump, drop_ann([20.0], duration_s=25.0))
        for theta in ("0.5", "0.8"):
            t = block["thresholds"][theta]
            self.assertAlmostEqual(t["coverage"], 1.0)
            # crossing at 14.0, beat 0.5 s -> (20 - 14) / 0.5 = 12 beats
            self.assertAlmostEqual(t["median_lead_beats"], 12.0, places=3)
        self.assertEqual(block["n_false_alarms"], 0)

    def test_single_sample_blip_is_ignored(self):
        pairs = grid(0.0, 5.0, 0.0) + [(5.0, 0.9)] + grid(5.033, 20.0, 0.0)
        dump = SignalDump.load(write_dump([META, *predict_series(pairs)]))
        block = m_drops.predict_drop(dump, drop_ann([], duration_s=20.0))
        self.assertEqual(block["thresholds"]["0.5"]["n_crossings"], 0)

    def test_hysteresis_prevents_refire_until_rearm(self):
        pairs = (
            grid(0.0, 10.0, 0.0)
            + grid(10.0, 12.0, 0.9)
            + grid(12.0, 13.0, 0.42)  # above 0.5 - 0.15: still disarmed
            + grid(13.0, 15.0, 0.9)  # no second crossing
            + grid(15.0, 16.0, 0.2)  # below 0.35: re-arms
            + grid(16.0, 18.0, 0.9)  # second crossing
        )
        dump = SignalDump.load(write_dump([META, *predict_series(pairs)]))
        block = m_drops.predict_drop(dump, drop_ann([], duration_s=18.0))
        self.assertEqual(block["thresholds"]["0.5"]["n_crossings"], 2)

    def test_decimation_gap_two_samples_qualify(self):
        pairs = grid(0.0, 10.0, 0.0, hz=30.0) + [(10.0, 0.9), (10.2, 0.9), (11.0, 0.0)]
        dump = SignalDump.load(write_dump([META, *predict_series(pairs)]))
        block = m_drops.predict_drop(dump, drop_ann([], duration_s=11.0))
        self.assertEqual(block["thresholds"]["0.5"]["n_crossings"], 1)

    def test_false_alarm_when_no_drop_follows(self):
        # Crossing at 2.0: no drop in (2, 18]. Crossing at 14: drop 20 in (14, 30].
        pairs = (
            grid(0.0, 2.0, 0.0)
            + grid(2.0, 3.0, 0.9)
            + grid(3.0, 14.0, 0.0)
            + grid(14.0, 20.0, 0.9)
        )
        dump = SignalDump.load(write_dump([META, *predict_series(pairs)]))
        block = m_drops.predict_drop(dump, drop_ann([20.0], duration_s=25.0))
        self.assertEqual(block["n_false_alarms"], 1)

    def test_lead_aggregation_pools_across_tracks(self):
        blocks = [
            {
                "thresholds": {"0.5": {"lead_beats": [8.0, 12.0]}, "0.8": {"lead_beats": []}},
                "n_ref": 2, "n_false_alarms": 1, "duration_min": 2.0,
            },
            {
                "thresholds": {"0.5": {"lead_beats": [4.0]}, "0.8": {"lead_beats": [2.0]}},
                "n_ref": 2, "n_false_alarms": 0, "duration_min": 2.0,
            },
        ]
        agg = m_drops._aggregate_predict(blocks)
        self.assertAlmostEqual(agg["0.5"]["lead_beats"]["median"], 8.0)
        self.assertAlmostEqual(agg["0.5"]["coverage"], 0.75)
        self.assertAlmostEqual(agg["0.8"]["coverage"], 0.25)
        self.assertAlmostEqual(agg["false_alarms_per_min"], 0.25)


class TestStemsMetric(unittest.TestCase):
    def test_correlation_against_constructed_stems(self):
        import soundfile as sf

        sr, dur = 44100, 4.0
        t = np.arange(int(sr * dur)) / sr
        silent_first = t >= 1.0  # first second: every stem silent -> excluded

        td = Path(tempfile.mkdtemp())
        ramp = np.where(silent_first, (t - 1.0) / 3.0, 0.0)
        sf.write(td / "drums.wav", ramp * np.sin(2 * np.pi * 220 * t), sr)
        sf.write(
            td / "bass.wav",
            np.where(silent_first, 0.1, 0.0) * np.sin(2 * np.pi * 60 * t),
            sr,
        )
        for name in ("vocals.wav", "other.wav"):
            sf.write(
                td / name,
                np.where(silent_first, 0.2, 0.0) * np.sin(2 * np.pi * 440 * t),
                sr,
            )

        raw = {
            "schema": "fosfora-bench-annotation/v1",
            "dataset": "test",
            "track_id": "t1",
            "audio": {"path": "t1.wav", "duration_s": dur},
            "stems": {
                "drums": "drums.wav",
                "bass": "bass.wav",
                "vocals": "vocals.wav",
                "other": "other.wav",
            },
        }
        (td / "t1.json").write_text(json.dumps(raw), encoding="utf-8")
        ann = Annotations.load(td / "t1.json")

        # Proxy streams on a 30 Hz grid: drums proxy follows the ramp.
        lines = [META]
        for name, fn in (
            ("drums", lambda x: max(0.0, (x - 1.0) / 3.0)),
            ("bass", lambda x: 0.5 if x >= 1.0 else 0.0),
            ("melody", lambda x: 0.5 if x >= 1.0 else 0.0),
        ):
            lines += series(f"/fosfora/v1/stem/{name}/energy", grid(0.1, dur, fn))
        block = m_stems.stems(SignalDump.load(write_dump(lines)), ann)

        self.assertGreater(block["drums"]["spearman"], 0.99)
        self.assertGreater(block["drums"]["pearson"], 0.8)
        self.assertGreater(block["drums"]["n_excluded_silence"], 0)
        # bass/melody proxies are constant on the kept frames: no correlation claim
        self.assertIsNone(block["bass"]["pearson"])


class TestDatasetlib(unittest.TestCase):
    def test_checked_in_manifests_validate(self):
        for name in ("ballroom", "smc"):
            m = dl.load_manifest(name)
            self.assertEqual(m["dataset"], name)
        self.assertEqual(
            dl.source(dl.load_manifest("ballroom"), "audio")["kind"], "archive"
        )
        with self.assertRaises(dl.DatasetError):
            dl.source(dl.load_manifest("ballroom"), "nope")

    def test_check_pins(self):
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "f.bin"
            p.write_bytes(b"fosfora")
            got = dl.check_pins(p, dl.sha256_file(p), dl.md5_file(p))
            self.assertEqual(got["size"], 7)
            with self.assertRaises(dl.DatasetError):
                dl.check_pins(p, "0" * 64, None)
            with self.assertRaises(dl.DatasetError):
                dl.check_pins(p, None, "0" * 32)

    def test_download_file_url_with_pins(self):
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            src = td / "src.bin"
            src.write_bytes(b"payload")
            dest = td / "out" / "dest.bin"
            got = dl.download(src.as_uri(), dest, sha256=dl.sha256_file(src), quiet=True)
            self.assertEqual(got.read_bytes(), b"payload")
            # existing verified dest is a no-op; a bad pin refuses the file
            dl.download(src.as_uri(), dest, sha256=dl.sha256_file(src), quiet=True)
            dest2 = td / "out" / "dest2.bin"
            with self.assertRaises(dl.DatasetError):
                dl.download(src.as_uri(), dest2, sha256="0" * 64, quiet=True)
            self.assertFalse(dest2.exists())

    def test_status_roundtrip_and_coverage(self):
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            st = dl.Status(td / "status.json", "test")
            f = td / "raw" / "a.bin"
            f.parent.mkdir()
            f.write_bytes(b"x")
            st.record_file(td, f)
            st.track("t1", fetch="ok")
            st.track("t2", fetch="http 404")
            st.save()
            st2 = dl.Status(td / "status.json", "test")
            self.assertEqual(st2.outcomes("fetch"), {"t1": "ok", "t2": "http 404"})
            manifest = {"dataset": "test", "expected": {"tracks": 5},
                        "exclusions": [{"id": "t9", "reason": "dup"}]}
            report = dl.coverage_report(manifest, st2, "fetch")
            self.assertIn("1 of 5 ok", report)
            self.assertIn("1 failed", report)
            self.assertIn("t2: http 404", report)

    def test_bundle_and_index_interop_with_scorer(self):
        with tempfile.TemporaryDirectory() as td:
            norm = Path(td)
            bundle = {
                "schema": "fosfora-bench-annotation/v1",
                "dataset": "test",
                "track_id": "t1",
                "audio": {"path": "t1.flac", "duration_s": 10.0},
                "beats": [0.5, 1.0],
            }
            dl.write_bundle(norm, bundle)
            dl.write_index(norm, "test", [
                {"track_id": "t1", "audio": "t1.flac", "annotations": "t1.json"}
            ])
            tracks = load_index(norm / "index.json")
            ann = Annotations.load(tracks[0]["annotations"])
            np.testing.assert_allclose(ann.beats, [0.5, 1.0])


if __name__ == "__main__":
    unittest.main()
