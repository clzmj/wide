#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Compare two g9ctl diagnose captures without treating latency as state."""
import argparse
import json
import sys
from pathlib import Path


def load(path):
    try:
        data = json.loads(Path(path).read_text())
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read {path}: {exc}") from exc
    if data.get("schema_version") != 1 or not isinstance(data.get("display"), dict):
        raise ValueError(f"{path}: expected diagnose schema_version 1")
    return data


def valid_edid(value):
    if not isinstance(value, str) or len(value) < 256 or len(value) % 256:
        return False
    try:
        raw = bytes.fromhex(value)
    except ValueError:
        return False
    if raw[:8] != bytes.fromhex("00ffffffffffff00"):
        return False
    blocks = raw[126] + 1
    return len(raw) >= blocks * 128 and all(
        sum(raw[i:i + 128]) % 256 == 0 for i in range(0, blocks * 128, 128)
    )


def positive_number(value):
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value if value > 0 else None
    if isinstance(value, str) and value.isdigit():
        number = int(value)
        return number if number > 0 else None
    return None


def same_display(before, after):
    a, b = before["display"], after["display"]
    manufacturer_a, manufacturer_b = a.get("manufacturer"), b.get("manufacturer")
    serial_a, serial_b = positive_number(a.get("serial")), positive_number(b.get("serial"))
    if (isinstance(manufacturer_a, str) and manufacturer_a.strip()
            and isinstance(manufacturer_b, str) and manufacturer_b.strip()
            and serial_a is not None and serial_b is not None):
        return (manufacturer_a.strip().casefold(), serial_a) == (
            manufacturer_b.strip().casefold(), serial_b
        )
    edid_a, edid_b = a.get("edid_hex"), b.get("edid_hex")
    return valid_edid(edid_a) and valid_edid(edid_b) and edid_a.lower() == edid_b.lower()


def readings(capture):
    return {item["vcp"].lower(): item for item in capture.get("reads", [])}


def compare(before, after):
    if not same_display(before, after):
        raise ValueError("captures do not identify the same display (manufacturer/product/serial/EDID differ)")
    left, right = readings(before), readings(after)
    changes = []
    metadata_changes = {}
    for key in ("product_id", "edid_hex"):
        old, new = before["display"].get(key), after["display"].get(key)
        if old != new:
            metadata_changes[key] = {"before": old, "after": new}
    for code in sorted(left.keys() & right.keys()):
        a, b = left[code], right[code]
        old = {k: a.get(k) for k in ("status", "current", "max")}
        new = {k: b.get(k) for k in ("status", "current", "max")}
        if old != new:
            changes.append({"vcp": code, "before": old, "after": new})
    only_before = sorted(left.keys() - right.keys())
    only_after = sorted(right.keys() - left.keys())
    warnings = []
    if before.get("complete") is False:
        warnings.append("before capture is marked incomplete")
    if after.get("complete") is False:
        warnings.append("after capture is marked incomplete")
    if "product_id" in metadata_changes:
        warnings.append("display product ID changed; the same serial identifies this as the same monitor")
    if "edid_hex" in metadata_changes:
        warnings.append("EDID changed; display mode changes can alter EDID metadata")
    if only_before or only_after:
        warnings.append("captures cover different VCP codes; compare changes only for shared codes")
    return {"same_display": True, "before_label": before.get("label"),
            "after_label": after.get("label"), "coverage": {
                "only_before": only_before, "only_after": only_after,
                "compared_count": len(left.keys() & right.keys()),
            }, "metadata_changes": metadata_changes, "warnings": warnings,
            "changes": changes}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before")
    parser.add_argument("after")
    parser.add_argument("--json", action="store_true", help="emit machine-readable output")
    args = parser.parse_args()
    try:
        result = compare(load(args.before), load(args.after))
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(result, indent=2))
    elif not result["changes"]:
        print("No semantic VCP changes.")
    else:
        for change in result["changes"]:
            print(f"{change['vcp']}: {change['before']} -> {change['after']}")
    for warning in result["warnings"]:
        print(f"Warning: {warning}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
