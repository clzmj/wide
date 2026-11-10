import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "diagnostics" / "compare-states.py"
SPEC = importlib.util.spec_from_file_location("compare_states", SCRIPT)
compare_states = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compare_states)


def capture(registry_id="display-1", current=50, latency=0.1):
    return {
        "schema_version": 1,
        "label": str(current),
        "display": {"registry_id": registry_id, "manufacturer": "SAM",
                    "product_id": 29851, "serial": 42, "edid_hex": "aabb"},
        "reads": [{"vcp": "0x10", "status": "ok", "current": current,
                   "max": 100, "attempts": [{"latency_ms": latency}]}],
    }


def valid_edid():
    block = bytearray(128)
    block[:8] = bytes.fromhex("00ffffffffffff00")
    block[8:10] = bytes.fromhex("4c2d")
    block[10:12] = bytes.fromhex("0100")
    block[127] = (-sum(block[:127])) % 256
    return block.hex()


class CompareTests(unittest.TestCase):
    def test_mode_change_detected_and_registry_and_latency_ignored(self):
        result = compare_states.compare(capture(), capture("new-id", 75, 99))
        self.assertEqual(result["changes"], [{
            "vcp": "0x10", "before": {"status": "ok", "current": 50, "max": 100},
            "after": {"status": "ok", "current": 75, "max": 100},
        }])

    def test_same_mode_has_no_semantic_change(self):
        self.assertEqual(compare_states.compare(capture(), capture("other", 50, 200))["changes"], [])

    def test_identity_mismatch_is_rejected(self):
        changed = capture()
        changed["display"]["serial"] = "different"
        with self.assertRaisesRegex(ValueError, "same display"):
            compare_states.compare(capture(), changed)

    def test_unknown_identity_is_rejected(self):
        unknown = capture()
        for key in ("manufacturer", "product_id", "serial"):
            unknown["display"][key] = None
        with self.assertRaisesRegex(ValueError, "same display"):
            compare_states.compare(unknown, unknown)

    def test_different_edid_is_rejected(self):
        changed = capture()
        for item in (changed["display"],):
            item["manufacturer"] = None
            item["product_id"] = None
            item["serial"] = None
        changed["display"]["edid_hex"] = valid_edid()
        before = capture()
        for item in (before["display"],):
            item["manufacturer"] = None
            item["product_id"] = None
            item["serial"] = None
        with self.assertRaisesRegex(ValueError, "same display"):
            compare_states.compare(before, changed)

    def test_valid_edid_can_identify_capture_without_serial_fields(self):
        first, second = capture(), capture("other-id")
        for item in (first, second):
            for key in ("manufacturer", "product_id", "serial"):
                item["display"][key] = None
            item["display"]["edid_hex"] = valid_edid()
        self.assertTrue(compare_states.compare(first, second)["same_display"])

    def test_native_numeric_identity_matches_across_edid_mode_change(self):
        before, after = capture(), capture("other-id")
        before["display"]["edid_hex"] = None
        after["display"]["edid_hex"] = valid_edid()
        self.assertTrue(compare_states.compare(before, after)["same_display"])

    def test_native_baseline_identity_matches(self):
        baseline = capture()
        baseline["label"] = "baseline"
        mode = capture("different-registry", 75)
        mode["label"] = "mode"
        self.assertTrue(compare_states.compare(baseline, mode)["same_display"])

    def test_real_pbp_product_id_change_keeps_same_serial_identity(self):
        baseline, pbp = capture(), capture("4295579489")
        baseline["display"].update(product_id=29851, serial=42)
        pbp["display"].update(product_id=29790, serial=42)
        result = compare_states.compare(baseline, pbp)
        self.assertTrue(result["same_display"])
        self.assertEqual(result["metadata_changes"]["product_id"],
                         {"before": 29851, "after": 29790})
        self.assertTrue(any("product ID changed" in warning for warning in result["warnings"]))

    def test_subset_scan_reports_coverage_without_state_changes(self):
        baseline = capture()
        baseline["reads"] = [
            {"vcp": f"0x{code:02x}", "status": "ok", "current": code,
             "max": 100, "attempts": []}
            for code in range(256)
        ]
        subset = dict(baseline)
        subset["reads"] = baseline["reads"][:74]
        result = compare_states.compare(baseline, subset)
        self.assertEqual(result["changes"], [])
        self.assertEqual(len(result["coverage"]["only_before"]), 182)
        self.assertEqual(result["coverage"]["only_after"], [])
        self.assertEqual(result["coverage"]["compared_count"], 74)
        self.assertTrue(result["warnings"])


if __name__ == "__main__":
    unittest.main()
