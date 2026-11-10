import importlib.util
import json
import tempfile
from types import SimpleNamespace
from collections import Counter
from pathlib import Path
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).parents[1] / "diagnostics" / "full-matrix.py"
SPEC = importlib.util.spec_from_file_location("full_matrix", SCRIPT)
full_matrix = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(full_matrix)


class MatrixTests(unittest.TestCase):
    def test_euler_tour_has_each_ordered_pair_once_and_required_triple(self):
        edges = full_matrix.matrix()
        self.assertEqual(len(edges), 25)
        self.assertEqual(Counter(edges), Counter(
            (a, b) for a in full_matrix.STATES for b in full_matrix.STATES))
        self.assertEqual(edges[0][0], "split-right")
        self.assertEqual(edges[-1][1], "split-right")
        self.assertTrue(all(a[1] == b[0] for a, b in zip(edges, edges[1:])))
        i = edges.index(("pip-small", "fullscreen"))
        self.assertEqual(edges[i + 1], ("fullscreen", "split-right"))

    def test_dry_run_requires_no_execute_and_has_no_binary_call(self):
        plan = full_matrix.plan(type("Args", (), {"start": "split-right"})())
        self.assertEqual(len(plan), 25)
        self.assertEqual(plan[0]["from"], "split-right")
        self.assertEqual(plan[-1]["to"], "split-right")
        self.assertTrue(all(step["max_writes"]["video"] <= 4 for step in plan))
        self.assertTrue(all(step["max_writes"]["audio"] <= 1 for step in plan))

    def test_expected_plan_for_pip_small_to_fullscreen_then_split_right(self):
        before = {"enabled": True, "mode": 7, "main": "display_port"}
        self.assertEqual(full_matrix.expected_steps(before, "fullscreen", "hdmi1"),
                         ["swap_sources", "disable"])
        after_fullscreen = {"enabled": False, "mode": 7, "main": "hdmi1"}
        self.assertEqual(full_matrix.expected_steps(after_fullscreen, "split-right", "hdmi1"),
                         ["restore_pbp", "equal_split", "enable", "swap_sources"])

    def test_fullscreen_with_disabled_wrong_main_disables_after_swap(self):
        before = {"enabled": False, "mode": 7, "main": "display_port"}
        self.assertEqual(full_matrix.expected_steps(before, "fullscreen", "hdmi1"),
                         ["enable", "swap_sources", "disable"])

    def test_expected_audio_role_value(self):
        self.assertEqual(full_matrix.expected_e8("split-left", 1), 0)
        self.assertEqual(full_matrix.expected_e8("pip-small", 0), 1)
        self.assertIsNone(full_matrix.expected_e8("fullscreen", 1))

    def test_pip_main_allows_observed_ultrawide_limit_without_relaxing_fullscreen(self):
        status = {"local_mode": {"id": 66, "pixel_width": 3840, "pixel_height": 1080, "refresh_hz": 120}}
        self.assertTrue(full_matrix.verify_resolution(status, "pip-big")[0])
        self.assertFalse(full_matrix.verify_resolution(status, "fullscreen")[0])

    def test_default_main_is_dry_and_never_launches_binary(self):
        with patch.object(full_matrix.subprocess, "run", side_effect=AssertionError("binary launched")):
            self.assertEqual(full_matrix.main([]), 0)

    def test_failure_stops_before_the_next_apply_without_hardware(self):
        class FailingRunner:
            calls = 0
            def __init__(self, *_):
                pass
            def call(self, name, args):
                if name == "status":
                    return {"display": {"manufacturer":"SAM", "serial":42,
                        "name":"Odyssey G93SC", "product_id":0x745e, "location":"External",
                        "registry_id":1, "cg_display_id":2},
                        "state":{"enabled":True,"mode":0,"main":"display_port",
                                 "secondary":"hdmi1","raw_e2":0x3511},
                        "local_mode":{"id":38,"pixel_width":2560,"pixel_height":1440,"refresh_hz":120}}
                if name == "read-e8":
                    return {"status":"ok", "max":2, "current":0,
                            "attempts":[{"write_status":0,"read_status":0}]}
                if name.startswith("apply-"):
                    FailingRunner.calls += 1
                    return {"ok":False}
                raise AssertionError(name)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fake-binary"
            binary.write_bytes(b"offline-only")
            args = SimpleNamespace(confirm="FULL-MATRIX", output=root/"capture", binary=binary,
                                   expected_serial=42, port="hdmi1")
            steps = full_matrix.plan(SimpleNamespace(start="split-right"))[:2]
            with patch.object(full_matrix, "Runner", FailingRunner):
                self.assertEqual(full_matrix.execute(args, steps), 1)
            self.assertEqual(FailingRunner.calls, 1)
            saved = json.loads((args.output/"progress.json").read_text())
            self.assertEqual(saved["status"], "failed")
            self.assertEqual(len(saved["results"]), 0)
            self.assertEqual(saved["active_edge"]["phase"], "failed_unknown_outcome")

    def test_resume_continues_after_verified_prefix_without_reapplying_it(self):
        state = {"enabled": False, "mode": 7, "main": "hdmi1",
                 "secondary": "display_port", "raw_e2": 0x35CE}
        mode = {"id": 38, "pixel_width": 5120, "pixel_height": 1440, "refresh_hz": 60.0}
        plan = [
            {"index": 1, "from": "split-right", "to": "fullscreen",
             "max_writes": {"video": 3, "audio": 0}},
            {"index": 2, "from": "fullscreen", "to": "fullscreen",
             "max_writes": {"video": 0, "audio": 0}},
        ]
        prefix = {"index": 1, "from": "split-right", "to": "fullscreen", "status": "verified",
                  "after": full_matrix.signature(state), "e8_after": 0,
                  "apply": {"resolution": {"after": mode}}}

        class FakeRunner:
            apply_calls = 0
            def __init__(self, _binary, _output):
                pass
            def call(self, name, arguments):
                if name == "status":
                    return {"display": {"manufacturer": "SAM", "serial": 42,
                        "name": "Odyssey G93SC", "product_id": 0x745E, "location": "External",
                        "registry_id": 1, "cg_display_id": 2}, "state": state, "local_mode": mode}
                if name == "read-e8":
                    raise AssertionError("E8 is cached and inapplicable in fullscreen")
                if name.startswith("modes-"):
                    return {"display": {"manufacturer":"SAM", "serial":42,
                        "name":"Odyssey G93SC", "product_id":0x749B, "location":"External",
                        "registry_id":1, "cg_display_id":2}, "current":mode}
                if name == "apply-02":
                    FakeRunner.apply_calls += 1
                    log = Path(arguments[arguments.index("--log") + 1])
                    log.write_text("")
                    return {"ok": True, "after":state, "plan": {"before": state, "steps": []}, "steps": [],
                            "recoveries": [], "audio": None,
                            "resolution": {"changed": False, "after": mode}}
                raise AssertionError(name)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fake-binary"
            binary.write_bytes(b"offline-only")
            prior = {"status": "failed", "port": "hdmi1", "expected_serial": 42,
                     "plan": plan, "results": [prefix], "binary_sha256": "priorhash",
                     "active_edge": {"index": 2, "from": "fullscreen", "to": "fullscreen",
                                     "phase": "failed_before_apply"}}
            resume = root / "prior-progress.json"
            resume.write_text(json.dumps(prior))
            args = SimpleNamespace(confirm="FULL-MATRIX", output=root / "new-run", binary=binary,
                                   expected_serial=42, port="hdmi1", resume=resume,
                                   controller_readbacks=True)
            with patch.object(full_matrix, "Runner", FakeRunner):
                self.assertEqual(full_matrix.execute(args, plan), 0)
            self.assertEqual(FakeRunner.apply_calls, 1)
            saved = json.loads((args.output / "progress.json").read_text())
            self.assertEqual(saved["status"], "complete_with_interruption")
            self.assertTrue(saved["complete_with_interruption"])
            self.assertEqual([item["index"] for item in saved["results"]], [1, 2])
            self.assertEqual(saved["resume"]["prior_binary_sha256"], "priorhash")
            self.assertIsNone(saved["results"][1]["e8_after"])
            self.assertIsNone(saved["results"][1]["e8_verified"])
            self.assertEqual(saved["results"][1]["e8_status"], "not_applicable_fullscreen")

    def test_resume_rejects_edge_with_unknown_apply_outcome(self):
        steps = full_matrix.plan(SimpleNamespace(start="split-right"))
        prior = {"status": "failed", "port": "hdmi1", "expected_serial": 42,
                 "plan": steps, "results": [{"index": 1, "from": steps[0]["from"],
                 "to": steps[0]["to"], "status": "verified"}],
                 "active_edge": {"index": 2, "from": steps[1]["from"],
                                 "to": steps[1]["to"], "phase": "failed_unknown_outcome"}}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "progress.json"
            path.write_text(json.dumps(prior))
            with self.assertRaisesRegex(ValueError, "possibly applied"):
                full_matrix.validate_resume(path, steps, "hdmi1", 42)

    def test_controller_readbacks_use_modes_after_apply_without_repolling_vcp(self):
        state = {"enabled": True, "mode": 0, "main": "display_port",
                 "secondary": "hdmi1", "raw_e2": 0x3511}
        mode = {"id": 38, "pixel_width": 2560, "pixel_height": 1440, "refresh_hz": 120.0}
        display = {"manufacturer": "SAM", "serial": 42, "name": "Odyssey G93SC",
                   "product_id": 0x745E, "location": "External", "cg_display_id": 2}

        class FakeRunner:
            calls = []
            apply_args = None
            def __init__(self, _binary, _output):
                pass
            def call(self, name, arguments):
                FakeRunner.calls.append(name)
                if name == "status":
                    return {"display": {**display, "registry_id": 1}, "state": state,
                            "local_mode": mode}
                if name == "read-e8":
                    return {"status": "ok", "max": 2, "current": 0,
                            "attempts": [{"write_status": 0, "read_status": 0}]}
                if name.startswith("modes-"):
                    registry = 11 if "before" in name else 12
                    return {"display": {**display, "registry_id": registry}, "current": mode}
                if name == "apply-01":
                    FakeRunner.apply_args = arguments
                    log = Path(arguments[arguments.index("--log") + 1])
                    log.write_text("")
                    audio = {"ok": True, "setting_writes": 0, "write": None,
                             "preflight": {"status": "ok", "max": 2, "current": 0,
                                           "attempts": [{"write_status": 0, "read_status": 0}]}}
                    return {"ok": True, "plan": {"before": state, "steps": []}, "steps": [],
                            "recoveries": [], "audio": audio, "after": state,
                            "resolution": {"changed": False, "after": mode}}
                raise AssertionError(name)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fake-binary"
            binary.write_bytes(b"offline-only")
            output = root / "run"
            args = SimpleNamespace(confirm="FULL-MATRIX", output=output, binary=binary,
                                   expected_serial=42, port="hdmi1", resume=None,
                                   controller_readbacks=True)
            edge = {"index": 1, "from": "split-right", "to": "split-right",
                    "max_writes": {"video": 0, "audio": 0}}
            with patch.object(full_matrix, "Runner", FakeRunner):
                self.assertEqual(full_matrix.execute(args, [edge]), 0)
            self.assertEqual(FakeRunner.calls[2:], ["modes-before-01", "apply-01", "modes-after-01"])
            self.assertEqual(FakeRunner.apply_args[0], "--display")
            self.assertEqual(FakeRunner.apply_args[1], "11")
            saved = json.loads((output / "progress.json").read_text())
            self.assertEqual(saved["verification_mode"],
                             "controller_readbacks_plus_independent_coregraphics")
            provenance = saved["results"][0]["readback_provenance"]
            self.assertEqual(provenance["post_state"], "native_apply_result.after")
            self.assertEqual(provenance["local_mode"], "fresh_passive_wide_advanced_modes")

    def test_start_at_fullscreen_self_edge_is_a_segment_without_e8_reads(self):
        state = {"enabled": False, "mode": 0, "main": "hdmi1",
                 "secondary": "display_port", "raw_e2": 0x35C0}
        mode = {"id": 38, "pixel_width": 5120, "pixel_height": 1440, "refresh_hz": 60.0}
        display = {"manufacturer": "SAM", "serial": 42, "name": "Odyssey G93SC",
                   "product_id": 0x7461, "location": "External", "cg_display_id": 2}

        class FakeRunner:
            calls = []
            def __init__(self, _binary, _output):
                pass
            def call(self, name, arguments):
                FakeRunner.calls.append(name)
                if name == "status":
                    return {"display": {**display, "registry_id": 12}, "state": state,
                            "local_mode": mode}
                if name.startswith("read-"):
                    raise AssertionError(f"fullscreen must not query cached E8: {name}")
                if name.startswith("modes-"):
                    registry = 13 if "before" in name else 14
                    return {"display": {**display, "registry_id": registry}, "current": mode}
                if name == "apply-02":
                    log = Path(arguments[arguments.index("--log") + 1])
                    log.write_text("")
                    return {"ok": True, "after": state, "plan": {"before": state, "steps": []},
                            "steps": [], "recoveries": [], "audio": None,
                            "resolution": {"changed": False, "after": mode}}
                raise AssertionError(name)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fake-binary"
            binary.write_bytes(b"offline-only")
            args = SimpleNamespace(confirm="FULL-MATRIX", output=root / "segment", binary=binary,
                                   expected_serial=42, port="hdmi1", resume=None,
                                   controller_readbacks=True, start_at=2)
            steps = full_matrix.plan(SimpleNamespace(start="split-right"))[:2]
            with patch.object(full_matrix, "Runner", FakeRunner):
                self.assertEqual(full_matrix.execute(args, steps), 0)
            self.assertEqual(FakeRunner.calls,
                             ["status", "modes-before-02", "apply-02", "modes-after-02"])
            saved = json.loads((args.output / "progress.json").read_text())
            self.assertEqual(saved["status"], "complete_segment")
            self.assertEqual(saved["verified_count"], 1)
            self.assertFalse(saved["full_matrix_complete"])
            self.assertEqual(saved["covered_indexes"], [2])

    def test_execution_requires_confirmation_before_creating_a_runner(self):
        with patch.object(full_matrix, "Runner", side_effect=AssertionError("runner created")):
            with self.assertRaises(ValueError):
                full_matrix.execute(SimpleNamespace(confirm=None), [])


if __name__ == "__main__":
    unittest.main()
