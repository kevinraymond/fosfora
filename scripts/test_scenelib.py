#!/usr/bin/env python3
"""Unit tests for scenelib's pure functions: the screenplay grammar, the
structural checks against analysis.json, the cue-timing math and the remap
calibration. Plain stdlib unittest, run as:

    python3 scripts/test_scenelib.py
"""

from __future__ import annotations

import copy
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import scenelib as sl


def fixture_analysis() -> dict:
    """Two sections, self-consistent with FIXTURE_SCREENPLAY below."""
    return {
        "version": 1,
        "source": {
            "path": "/tmp/fake.mp3", "duration_secs": 60.0, "sample_rate": 44100.0,
            "source_channels": 2, "hop_hz": 86.13, "hop_count": 5168,
        },
        "global": {
            "bpm": 120.0, "beat_count": 120, "downbeat_count": 30, "key_class": 0,
            "key_is_minor": False, "key_agreement": 0.8, "loudness_s_mean": 0.5,
            "cluster_count": 2,
        },
        "sections": [
            {
                "index": 0, "start_secs": 0.0, "end_secs": 30.0, "duration_secs": 30.0,
                "label": "A", "cluster": 0, "energy": 0.4, "energy_rank": 0.0,
                "fingerprint": [0.0] * 27,
                "descriptors": {
                    "rms": 0.3, "centroid": 0.4, "percussive_energy": 0.2,
                    "harmonic_ratio": 0.7, "buildup": 0.1, "stereo_width": 0.2,
                    "onset_density": 2.0,
                },
                "percentiles": {
                    "rms": [0.2, 0.3, 0.4],
                    "sub_bass": [0.1, 0.2, 0.3],
                    "buildup": [0.5, 0.5, 0.5],
                },
            },
            {
                "index": 1, "start_secs": 30.0, "end_secs": 60.0, "duration_secs": 30.0,
                "label": "B", "cluster": 1, "energy": 0.7, "energy_rank": 1.0,
                "fingerprint": [0.0] * 27,
                "descriptors": {
                    "rms": 0.6, "centroid": 0.5, "percussive_energy": 0.5,
                    "harmonic_ratio": 0.4, "buildup": 0.3, "stereo_width": 0.4,
                    "onset_density": 3.5,
                },
                "percentiles": {
                    "rms": [0.5, 0.6, 0.7],
                    "sub_bass": [0.4, 0.5, 0.6],
                    "buildup": [0.5, 0.5, 0.5],
                },
            },
        ],
        "events": {
            "beats_secs": [], "downbeats_secs": [], "drops_secs": [8.0],
            "boundaries_secs": [30.0],
        },
    }


FIXTURE_SCREENPLAY = """\
# The Test Song — A Screenplay

`[song | 60.0s | 120.0 BPM | key C major | 2 sections / 2 identities | drops: 8.0s]`

## Act I — Openings

### Scene 0 — First Light

`[section 0 | 0.0s – 30.0s | 30.0s | identity A | energy 0.40, rank 1/2 | drops: 8.0s]`
`[signals: rms 0.30 · centroid 0.40 · percussive 0.20 · onsets 2.0/s · harmonic 0.70 · width 0.20 · buildup 0.10]`

A slow dawn over still water.

**Beats**
- **0a (0.0 – 15.0) — Dawn.** Light gathers at the horizon.
- **0b (15.0 – 30.0) — Ripple.** The drop at 8.0 was a stone; now the rings arrive.

**Direction** — pace: drifting · light: dim, warming · palette: slate blue to amber · stillness: near-total

### Scene 1 — The Turn

`[section 1 | 30.0s – 60.0s | 30.0s | identity B | energy 0.70, rank 2/2 | drops: none]`
`[signals: rms 0.60 · centroid 0.50 · percussive 0.50 · onsets 3.5/s · harmonic 0.40 · width 0.40 · buildup 0.30]`

The kick arrives and the water becomes weather.

**Direction** — pace: driving · light: hard, flashing · palette: white on black · stillness: none
"""


class ParseTests(unittest.TestCase):
    def test_round_trip_of_the_fixture(self):
        sp = sl.parse_screenplay(FIXTURE_SCREENPLAY)
        self.assertEqual(sp.parse_problems, [])
        self.assertEqual(sp.song["duration_secs"], 60.0)
        self.assertEqual(sp.song["bpm"], 120.0)
        self.assertEqual(sp.song["key"], "C major")
        self.assertEqual(sp.song["drops"], [8.0])
        self.assertEqual(len(sp.scenes), 2)

        s0 = sp.scenes[0]
        self.assertEqual((s0.index, s0.start, s0.end, s0.label), (0, 0.0, 30.0, "A"))
        self.assertEqual(s0.energy, 0.40)
        self.assertEqual((s0.rank, s0.rank_of), (1, 2))
        self.assertEqual(s0.drops, [8.0])
        self.assertEqual([b.id for b in s0.beats], ["0a", "0b"])
        self.assertEqual(s0.beats[0].title, "Dawn")
        self.assertEqual(s0.beats[1].prose, "The drop at 8.0 was a stone; now the rings arrive.")
        self.assertEqual(s0.direction["pace"], "drifting")
        self.assertEqual(s0.direction["palette"], "slate blue to amber")

        s1 = sp.scenes[1]
        self.assertEqual(s1.drops, [])
        self.assertEqual(s1.beats, [])
        # No Beats block => one implicit beat spanning the section.
        eff = s1.effective_beats()
        self.assertEqual(len(eff), 1)
        self.assertTrue(eff[0].implicit)
        self.assertEqual((eff[0].start, eff[0].end), (30.0, 60.0))

    def test_title_headed_song_line_is_accepted(self):
        text = FIXTURE_SCREENPLAY.replace("`[song |", "`[The Test Song |")
        sp = sl.parse_screenplay(text)
        self.assertEqual(sp.parse_problems, [])
        self.assertEqual(sp.song["duration_secs"], 60.0)
        # And normalization rewrites it to the canonical spelling.
        out = sl.normalize_screenplay(sp, fixture_analysis())
        self.assertIn("`[song | 60.0s | 120.0 BPM", out)

    def test_hyphen_dashes_and_bare_seconds_are_accepted(self):
        text = FIXTURE_SCREENPLAY.replace("0.0s – 30.0s", "0.0 - 30.0").replace(
            "(0.0 – 15.0)", "(0.0 - 15.0)"
        )
        sp = sl.parse_screenplay(text)
        self.assertEqual(sp.parse_problems, [])
        self.assertEqual(sp.scenes[0].end, 30.0)

    def test_malformed_section_line_is_a_parse_problem_not_prose(self):
        text = FIXTURE_SCREENPLAY.replace("identity A", "identity")
        sp = sl.parse_screenplay(text)
        self.assertTrue(any("malformed section line" in p for p in sp.parse_problems))

    def test_all_beats_spans_every_scene(self):
        sp = sl.parse_screenplay(FIXTURE_SCREENPLAY)
        beats = [b.id for _, b in sp.all_beats()]
        self.assertEqual(beats, ["0a", "0b", "1a"])


class CheckTests(unittest.TestCase):
    def check(self, text, analysis=None):
        return sl.check_screenplay(sl.parse_screenplay(text), analysis or fixture_analysis())

    def test_fixture_is_clean(self):
        problems, warnings = self.check(FIXTURE_SCREENPLAY)
        self.assertEqual(problems, [])
        self.assertEqual(warnings, [])

    def test_missing_section_is_fatal(self):
        text = "\n".join(
            ln for ln in FIXTURE_SCREENPLAY.splitlines() if "[section 1" not in ln
        )
        problems, _ = self.check(text)
        self.assertTrue(any("every section must appear exactly once" in p for p in problems))

    def test_retimed_section_boundary_is_fatal(self):
        text = FIXTURE_SCREENPLAY.replace(
            "`[section 1 | 30.0s – 60.0s", "`[section 1 | 32.0s – 60.0s"
        )
        problems, _ = self.check(text)
        self.assertTrue(any("cannot be retimed" in p for p in problems))

    def test_non_tiling_beats_are_fatal(self):
        text = FIXTURE_SCREENPLAY.replace("**0b (15.0 – 30.0)", "**0b (18.0 – 30.0)")
        problems, _ = self.check(text)
        self.assertTrue(any("must tile the section" in p for p in problems))

    def test_beat_shorter_than_a_cue_is_fatal(self):
        text = FIXTURE_SCREENPLAY.replace("(0.0 – 15.0)", "(0.0 – 0.5)").replace(
            "(15.0 – 30.0)", "(0.5 – 30.0)"
        )
        problems, _ = self.check(text)
        self.assertTrue(any("at least 2s" in p for p in problems))

    def test_bad_pace_word_is_fatal(self):
        text = FIXTURE_SCREENPLAY.replace("pace: driving", "pace: bombastic")
        problems, _ = self.check(text)
        self.assertTrue(any("'bombastic' is not one of" in p for p in problems))

    def test_missing_direction_is_fatal_missing_signals_is_a_warning(self):
        lines = [
            ln for ln in FIXTURE_SCREENPLAY.splitlines()
            if not ("**Direction**" in ln and "driving" in ln)
            and "rms 0.30" not in ln
        ]
        problems, warnings = self.check("\n".join(lines))
        self.assertTrue(any("no `**Direction**` line" in p for p in problems))
        self.assertTrue(any("no signals line" in w for w in warnings))

    def test_wrong_beat_ids_are_fatal(self):
        text = FIXTURE_SCREENPLAY.replace("**0b (", "**0c (")
        problems, _ = self.check(text)
        self.assertTrue(any("beat ids" in p for p in problems))


class NormalizeTests(unittest.TestCase):
    def test_numbers_snap_to_analysis_and_prose_survives(self):
        # Rounded-off numbers within tolerance, as a model plausibly emits.
        text = FIXTURE_SCREENPLAY.replace(
            "`[section 0 | 0.0s – 30.0s | 30.0s", "`[section 0 | 0.1s – 29.9s | 29.8s"
        ).replace("**0b (15.0 – 30.0)", "**0b (15.0 – 29.9)")
        sp = sl.parse_screenplay(text)
        problems, _ = sl.check_screenplay(sp, fixture_analysis())
        self.assertEqual(problems, [])

        out = sl.normalize_screenplay(sp, fixture_analysis())
        self.assertIn("`[section 0 | 0.0s – 30.0s | 30.0s | identity A", out)
        self.assertIn("(15.0 – 30.0)", out)  # last beat end snapped back
        self.assertIn("A slow dawn over still water.", out)
        self.assertIn("now the rings arrive.", out)
        # Idempotent on already-canonical text.
        sp2 = sl.parse_screenplay(out)
        self.assertEqual(sl.normalize_screenplay(sp2, fixture_analysis()), out)


class CueTimingTests(unittest.TestCase):
    def test_transitions_complete_on_beat_boundaries_and_telescope(self):
        spans = [(0.0, 15.0), (15.0, 30.0), (30.0, 60.0)]
        req = [
            {"transition": "Cut", "transition_secs": 0.0},
            {"transition": "Dissolve", "transition_secs": 2.0},
            {"transition": "ParamMorph", "transition_secs": 100.0},  # absurd, gets clamped
        ]
        cues = sl.plan_cue_timing(spans, req)
        self.assertEqual(cues[0], {"transition": "Cut", "transition_secs": 0.0, "hold_secs": 13.0})
        self.assertEqual(cues[1]["transition_secs"], 2.0)
        # Clamp: 25% of min(15, 30) = 3.75.
        self.assertEqual(cues[2]["transition_secs"], 3.75)
        self.assertEqual(cues[1]["hold_secs"], 15.0 - 3.75)
        self.assertEqual(cues[2]["hold_secs"], 30.0)
        # Sum of (transition + hold) over all cues telescopes to the song length.
        total = sum(c["transition_secs"] + c["hold_secs"] for c in cues)
        self.assertAlmostEqual(total, 60.0, places=2)

    def test_first_cue_is_forced_to_cut(self):
        cues = sl.plan_cue_timing(
            [(0.0, 10.0), (10.0, 20.0)],
            [{"transition": "Dissolve", "transition_secs": 5.0},
             {"transition": "Cut", "transition_secs": 0.0}],
        )
        self.assertEqual(cues[0]["transition"], "Cut")
        self.assertEqual(cues[0]["transition_secs"], 0.0)
        self.assertEqual(cues[0]["hold_secs"], 10.0)


class CalibrationTests(unittest.TestCase):
    def result_with(self, transforms, source="audio.rms"):
        return {
            "presets": [{
                "name": "P",
                "layers": [{"effect": "Aurora", "blend_mode": "Normal", "opacity": 1.0, "params": []}],
                "bindings": [{
                    "name": "b", "source": source,
                    "target": {"kind": "param", "layer": 0, "param": "intensity"},
                    "transforms": transforms,
                }],
            }],
            "cues": [],
        }

    def test_leading_remap_gets_pooled_p10_p90(self):
        r = self.result_with([{"type": "remap", "in_lo": 0, "in_hi": 1, "out_lo": 0.2, "out_hi": 0.9}])
        log, warnings = sl.calibrate_remaps(r, fixture_analysis(), {"P": [0, 1]})
        t = r["presets"][0]["bindings"][0]["transforms"][0]
        self.assertEqual((t["in_lo"], t["in_hi"]), (0.2, 0.7))  # min p10, max p90
        self.assertEqual((t["out_lo"], t["out_hi"]), (0.2, 0.9))  # model's, untouched
        self.assertEqual(len(log), 1)
        self.assertEqual(warnings, [])

    def test_only_covered_sections_pool(self):
        r = self.result_with([{"type": "remap", "in_lo": 0, "in_hi": 1, "out_lo": 0, "out_hi": 1}])
        sl.calibrate_remaps(r, fixture_analysis(), {"P": [1]})
        t = r["presets"][0]["bindings"][0]["transforms"][0]
        self.assertEqual((t["in_lo"], t["in_hi"]), (0.5, 0.7))

    def test_degenerate_width_gets_the_floor(self):
        r = self.result_with(
            [{"type": "remap", "in_lo": 0, "in_hi": 1, "out_lo": 0, "out_hi": 1}],
            source="audio.buildup",  # fixture percentiles are flat 0.5
        )
        sl.calibrate_remaps(r, fixture_analysis(), {"P": [0, 1]})
        t = r["presets"][0]["bindings"][0]["transforms"][0]
        self.assertAlmostEqual(t["in_hi"] - t["in_lo"], sl.MIN_CALIBRATED_WIDTH, places=3)

    def test_band_source_maps_to_band_feature(self):
        r = self.result_with(
            [{"type": "remap", "in_lo": 0, "in_hi": 1, "out_lo": 0, "out_hi": 1}],
            source="audio.band.0",
        )
        sl.calibrate_remaps(r, fixture_analysis(), {"P": [0, 1]})
        t = r["presets"][0]["bindings"][0]["transforms"][0]
        self.assertEqual((t["in_lo"], t["in_hi"]), (0.1, 0.6))  # sub_bass pooled

    def test_trigger_sources_are_left_alone(self):
        r = self.result_with(
            [{"type": "remap", "in_lo": 0.3, "in_hi": 0.8, "out_lo": 0, "out_hi": 1}],
            source="audio.beat",
        )
        log, warnings = sl.calibrate_remaps(r, fixture_analysis(), {"P": [0, 1]})
        t = r["presets"][0]["bindings"][0]["transforms"][0]
        self.assertEqual((t["in_lo"], t["in_hi"]), (0.3, 0.8))
        self.assertEqual((log, warnings), ([], []))

    def test_non_leading_remap_warns_and_is_untouched(self):
        r = self.result_with([
            {"type": "smooth", "factor": 0.5},
            {"type": "remap", "in_lo": 0.3, "in_hi": 0.8, "out_lo": 0, "out_hi": 1},
        ])
        log, warnings = sl.calibrate_remaps(r, fixture_analysis(), {"P": [0, 1]})
        t = r["presets"][0]["bindings"][0]["transforms"][1]
        self.assertEqual((t["in_lo"], t["in_hi"]), (0.3, 0.8))
        self.assertTrue(any("not first" in w for w in warnings))
        self.assertEqual(log, [])

    def test_analysis_without_percentiles_warns_once(self):
        analysis = fixture_analysis()
        for s in analysis["sections"]:
            del s["percentiles"]
        r = self.result_with([{"type": "remap", "in_lo": 0, "in_hi": 1, "out_lo": 0, "out_hi": 1}])
        log, warnings = sl.calibrate_remaps(r, analysis, {"P": [0, 1]})
        self.assertTrue(any("re-run --analyze" in w for w in warnings))
        self.assertEqual(log, [])


class CheckRunTests(unittest.TestCase):
    def setUp(self):
        self.sp = sl.parse_screenplay(FIXTURE_SCREENPLAY)  # beats 0a, 0b, 1a
        self.scene = {"cues": [
            {"transition_secs": 0.0}, {"transition_secs": 2.0}, {"transition_secs": 3.0},
        ]}

    def test_on_time_run_is_clean(self):
        run = {"cue_spans": [
            {"cue": 0, "start_secs": 0.0},
            {"cue": 1, "start_secs": 13.0},   # beat 0b starts 15.0, minus 2.0 transition
            {"cue": 2, "start_secs": 27.05},  # beat 1a starts 30.0, minus 3.0, timer slop
        ]}
        self.assertEqual(sl.check_run(run, self.scene, self.sp), [])

    def test_late_cue_is_flagged(self):
        run = {"cue_spans": [
            {"cue": 0, "start_secs": 0.0},
            {"cue": 1, "start_secs": 16.2},
            {"cue": 2, "start_secs": 27.0},
        ]}
        problems = sl.check_run(run, self.scene, self.sp)
        self.assertTrue(any("cue 1 (beat 0b)" in p for p in problems))

    def test_stalled_timeline_is_flagged(self):
        run = {"cue_spans": [{"cue": 0, "start_secs": 0.0}]}
        problems = sl.check_run(run, self.scene, self.sp)
        self.assertTrue(any("stalled or skipped" in p for p in problems))


class EmitHelperTests(unittest.TestCase):
    def test_assemble_target_forms(self):
        self.assertEqual(
            sl.assemble_target({"kind": "param", "layer": 1, "effect": "Aurora", "param": "speed"}),
            "param.1.Aurora.speed",
        )
        self.assertEqual(
            sl.assemble_target({"kind": "layer", "layer": 0, "layer_field": "opacity"}),
            "layer.0.opacity",
        )
        self.assertEqual(sl.assemble_target({"kind": "global_master_opacity"}), "global.master_opacity")
        self.assertEqual(sl.assemble_target({"kind": "postfx", "postfx": "bloom"}), "postfx.bloom")

    def test_param_value_tagging(self):
        self.assertEqual(sl.param_value({"name": "x", "type": "Float", "value": 0.5}), {"Float": 0.5})
        self.assertEqual(sl.param_value({"name": "x", "type": "Bool", "value": 1}), {"Bool": True})

    def test_check_variants_flags_incomplete_transform(self):
        result = {"presets": [{
            "name": "P", "layers": [], "bindings": [{
                "name": "b", "source": "audio.rms",
                "target": {"kind": "param", "layer": 0, "param": "x"},
                "transforms": [{"type": "deadzone"}],
            }],
        }]}
        problems = sl.check_variants(result)
        self.assertTrue(any("requires 'lo'" in p for p in problems))


if __name__ == "__main__":
    unittest.main()
