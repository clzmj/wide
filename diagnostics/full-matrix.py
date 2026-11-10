#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Plan, or explicitly execute, the 25 ordered layout transitions on a G93SC."""
import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

STATES = ("fullscreen", "split-left", "split-right", "pip-big", "pip-small")
S, F, R = "pip-small", "fullscreen", "split-right"
EXPECTED_PRODUCTS = {0x749B, 0x745E, 0x7462, 0x7461}
EXPECTED_RESOLUTION = {
    "fullscreen": (5120, 1440), "pip-big": (5120, 1440),
    "split-left": (2560, 1440), "split-right": (2560, 1440),
    "pip-small": (1920, 1080),
}
STEP_VALUES = {"enable": (0xE2, 1), "disable": (0xE2, 0),
               "restore_pbp": (0xE2, 2), "equal_split": (0xE2, 3),
               "select_pip": (0xE2, 16), "swap_sources": (0xE3, 4096)}


def matrix():
    """Build an Euler tour, contracting S→F→R and expanding it afterward."""
    vertices = list(STATES)
    edges = []
    for src in vertices:
        for dst in vertices:
            if (src, dst) not in ((S, F), (F, R)):
                edges.append((src, dst, False))
    edges.append((S, R, True))
    outgoing = {v: [] for v in vertices}
    for i, edge in enumerate(edges):
        outgoing[edge[0]].append(i)
    for choices in outgoing.values():
        choices.reverse()
    stack, circuit = [(R, None)], []
    used = set()
    while stack:
        src = stack[-1][0]
        while outgoing[src] and outgoing[src][-1] in used:
            outgoing[src].pop()
        if outgoing[src]:
            edge_id = outgoing[src].pop()
            used.add(edge_id)
            stack.append((edges[edge_id][1], edge_id))
        else:
            _, incoming = stack.pop()
            if incoming is not None:
                circuit.append(incoming)
    circuit.reverse()
    expanded = []
    for edge_id in circuit:
        src, dst, virtual = edges[edge_id]
        if virtual:
            expanded.extend(((S, F), (F, R)))
        else:
            expanded.append((src, dst))
    if len(expanded) != 25 or Counter(expanded) != Counter(
            (a, b) for a in STATES for b in STATES):
        raise RuntimeError("matrix construction did not produce each directed transition once")
    if (S, F) not in expanded or expanded[expanded.index((S, F)) + 1] != (F, R):
        raise RuntimeError("required pip-small → fullscreen → split-right sequence is absent")
    if expanded[0][0] != R or expanded[-1][1] != R:
        raise RuntimeError("matrix must start and end at split-right")
    if any(previous[1] != following[0] for previous, following in zip(expanded, expanded[1:])):
        raise RuntimeError("matrix edges do not form a continuous route")
    return expanded


def max_writes(edge):
    before, after = edge
    if before == after:
        return {"video": 0, "audio": 0}
    if after == "fullscreen":
        return {"video": 3, "audio": 0}
    return {"video": 4, "audio": 1}


def source_value(port):
    return "hdmi1" if port == "hdmi1" else "dp"


def set_packet(code, value):
    hi, lo = divmod(value, 256)
    packet = [0x84, 0x03, code, hi, lo]
    packet.append(0x6E ^ 0x51 ^ packet[0] ^ packet[1] ^ code ^ hi ^ lo)
    return bytes(packet).hex()


def role(state, port):
    main = state.get("main")
    secondary = state.get("secondary")
    if main not in ("hdmi1", "display_port") or secondary not in ("hdmi1", "display_port") or main == secondary:
        raise ValueError("monitor source pair is unknown or invalid")
    is_this = main == ("hdmi1" if port == "hdmi1" else "display_port")
    if state.get("enabled") is False:
        if not is_this:
            raise ValueError("full-screen main source is not this Mac's selected port")
        return "fullscreen"
    if state.get("mode") == 0:
        return "split-left" if is_this else "split-right"
    if state.get("mode") == 7:
        return "pip-big" if is_this else "pip-small"
    raise ValueError("monitor is in an unsupported mode")


def expected_steps(before, target, port):
    enabled, mode = before["enabled"], before["mode"]
    source_is_main = before["main"] in (("hdmi1",) if port == "hdmi1" else ("display_port", "dp"))
    target_main_is_this = target in ("fullscreen", "split-left", "pip-big")
    steps = []
    if target == "fullscreen":
        if source_is_main != target_main_is_this:
            if not enabled:
                steps.append("enable")
                enabled = True
            steps.append("swap_sources")
        if enabled:
            steps.append("disable")
    elif target in ("split-left", "split-right"):
        if mode == 7:
            steps.extend(("restore_pbp", "equal_split"))
        elif mode != 0:
            steps.append("equal_split")
        if not enabled:
            steps.append("enable")
        if source_is_main != target_main_is_this:
            steps.append("swap_sources")
    else:
        if mode != 7:
            steps.append("select_pip")
        if not enabled:
            steps.append("enable")
        if source_is_main != target_main_is_this:
            steps.append("swap_sources")
    return steps


def parser_for(project):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--execute", action="store_true", help="run hardware transitions (requires --confirm)")
    p.add_argument("--confirm", help="must be exactly FULL-MATRIX to execute")
    p.add_argument("--binary", type=Path, default=project / "target/aarch64-apple-darwin/release/wide")
    p.add_argument("--port", choices=("hdmi1", "dp"), default="hdmi1")
    p.add_argument("--expected-serial", type=int, help="required for hardware execution; use the selected monitor serial")
    p.add_argument("--start", choices=STATES, default="split-right")
    p.add_argument("--output", type=Path, help="new output directory; required for execution")
    p.add_argument("--resume", type=Path, help="continue after a verified prefix in a failed progress.json")
    p.add_argument("--start-at", type=int, default=1,
                   help="start a new segment at this 1-based matrix edge (default: 1)")
    p.add_argument("--controller-readbacks", action="store_true",
                   help="use native apply readbacks plus passive macOS mode checks between edges")
    return p


class Runner:
    def __init__(self, binary, output):
        self.binary = binary
        self.output = output
        self.counter = 0

    def call(self, name, args):
        self.counter += 1
        stem = f"{self.counter:02d}-{name}"
        try:
            done = subprocess.run([str(self.binary), "--json", *args], capture_output=True,
                                  text=True, timeout=150, check=False)
        except (OSError, subprocess.TimeoutExpired) as exc:
            (self.output / f"{stem}.stderr.txt").write_text(str(exc) + "\n")
            raise RuntimeError(f"{name} command could not complete: {exc}") from exc
        (self.output / f"{stem}.stdout.txt").write_text(done.stdout)
        (self.output / f"{stem}.stderr.txt").write_text(done.stderr)
        if done.returncode:
            raise RuntimeError(f"{name} exited with status {done.returncode}")
        try:
            return json.loads(done.stdout)
        except json.JSONDecodeError as exc:
            raise RuntimeError(f"{name} did not return JSON") from exc


def display(status, expected_serial, port):
    d = status.get("display", {})
    if (d.get("manufacturer") != "SAM" or d.get("serial") != expected_serial
            or "G93SC" not in d.get("name", "").upper()
            or d.get("product_id") not in EXPECTED_PRODUCTS
            or d.get("location") != "External"
            or not isinstance(d.get("registry_id"), int) or d["registry_id"] <= 0
            or not isinstance(d.get("cg_display_id"), int) or d["cg_display_id"] <= 0):
        raise RuntimeError("fresh status does not identify the expected Samsung serial")
    known = d.get("connection")
    if known and known.lower() not in (("hdmi", "hdmi1") if port == "hdmi1" else ("displayport", "dp")):
        raise RuntimeError("detected connection conflicts with --port")
    return d


def read_code(runner, registry_id, code):
    value = runner.call(f"read-{code:02x}", ["--display", str(registry_id), "advanced", "ddc", "read", f"0x{code:02X}"])
    if (value.get("status") != "ok" or value.get("max") != 2
            or type(value.get("current")) is not int or value["current"] not in (0, 1)
            or not value.get("attempts")
            or value["attempts"][-1].get("write_status") != 0
            or value["attempts"][-1].get("read_status") != 0):
        raise RuntimeError(f"VCP 0x{code:02X} read did not return an integer value")
    return value["current"]


def observe(runner, expected_serial, port):
    status = runner.call("status", ["status"])
    d = display(status, expected_serial, port)
    # Status reads and decodes E2/E3. Firmware E8 is only meaningful while PBP/PiP is enabled.
    state = status.get("state")
    if not isinstance(state, dict):
        raise RuntimeError("status omitted decoded monitor state")
    e2 = state.get("raw_e2")
    if not isinstance(e2, int):
        raise RuntimeError("status omitted the fresh E2 value")
    source_codes = {"hdmi1": 0, "display_port": 16, "dp": 16}
    expected_pair = ((source_codes.get(state.get("main"), -1) << 8)
                     | source_codes.get(state.get("secondary"), -1))
    if expected_pair < 0:
        raise RuntimeError("status omitted the fresh E3 source pair")
    e3 = expected_pair
    e8_applicable = state.get("enabled") is True
    e8 = read_code(runner, d["registry_id"], 0xE8) if e8_applicable else None
    return {"status": status, "display": d, "e2": e2, "e3": e3, "e8": e8,
            "e8_applicable": e8_applicable, "state": state}


def observe_mode(runner, expected_serial, port, name):
    value = runner.call(name, ["advanced", "modes"])
    d = display(value, expected_serial, port)
    mode = value.get("current")
    pseudo_status = {"local_mode": mode}
    refresh = mode.get("refresh_hz") if isinstance(mode, dict) else None
    if (not isinstance(mode, dict) or type(mode.get("id")) is not int or mode["id"] < 0
            or type(mode.get("pixel_width")) is not int or mode["pixel_width"] <= 0
            or type(mode.get("pixel_height")) is not int or mode["pixel_height"] <= 0
            or type(refresh) not in (int, float) or not math.isfinite(refresh) or refresh <= 0):
        raise RuntimeError("passive modes command omitted current mode identity")
    return {"display": d, "status": pseudo_status, "mode": mode}


def state_e3(state):
    source_codes = {"hdmi1": 0, "display_port": 16}
    if state.get("main") not in source_codes or state.get("secondary") not in source_codes:
        raise RuntimeError("apply result omitted a valid source pair")
    return (source_codes[state["main"]] << 8) | source_codes[state["secondary"]]


def audio_record_value(record):
    if (not isinstance(record, dict) or record.get("status") != "ok" or record.get("max") != 2
            or type(record.get("current")) is not int or record["current"] not in (0, 1)
            or not record.get("attempts") or record["attempts"][-1].get("write_status") != 0
            or record["attempts"][-1].get("read_status") != 0):
        raise RuntimeError("apply audio readback is missing or invalid")
    return record["current"]


def signature(state):
    return {k: state.get(k) for k in ("enabled", "mode", "main", "secondary", "raw_e2")}


def expected_e8(target, before):
    return None if target == "fullscreen" else (0 if target.startswith("split-") else 1)


def verify_resolution(status, target):
    mode = status.get("local_mode") or {}
    observed = (mode.get("pixel_width"), mode.get("pixel_height"))
    refresh = mode.get("refresh_hz")
    valid = (type(mode.get("id")) is int and mode["id"] >= 0
             and type(refresh) in (int, float) and math.isfinite(refresh) and refresh >= 0)
    allowed = (((5120, 1440), (3840, 1080)) if target == "pip-big" else
               ((1920, 1080), (1280, 720)) if target == "pip-small" else (EXPECTED_RESOLUTION[target],))
    return valid and observed in allowed, observed


def mode_signature(status):
    mode = status.get("local_mode") or {}
    return {key: mode.get(key) for key in ("id", "pixel_width", "pixel_height", "refresh_hz")}


def plan(args):
    edges = matrix()
    if args.start != "split-right":
        raise ValueError("hardware-matrix execution requires --start split-right")
    return [{"index": n, "from": a, "to": b, "max_writes": max_writes((a, b))}
            for n, (a, b) in enumerate(edges, 1)]


def persist(output, value):
    tmp = output / ".progress.tmp"
    tmp.write_text(json.dumps(value, indent=2) + "\n")
    os.replace(tmp, output / "progress.json")


def validate_resume(path, steps, port, expected_serial):
    try:
        prior = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read resume progress: {exc}") from exc
    if prior.get("status") != "failed":
        raise ValueError("resume requires prior progress status=failed")
    if prior.get("port") != port or prior.get("expected_serial") != expected_serial:
        raise ValueError("resume port or expected serial differs from prior run")
    if prior.get("plan") != steps:
        raise ValueError("resume plan does not exactly match the current 25-edge matrix")
    results = prior.get("results")
    if not isinstance(results, list) or not results or len(results) >= len(steps):
        raise ValueError("resume requires a nonempty, incomplete verified prefix")
    for index, result in enumerate(results):
        expected = steps[index]
        if (result.get("status") != "verified" or result.get("index") != index + 1
                or result.get("from") != expected["from"] or result.get("to") != expected["to"]):
            raise ValueError("resume results are not a verified prefix of the current matrix")
    active = prior.get("active_edge")
    if active is not None:
        next_step = steps[len(results)]
        if (active.get("phase") != "failed_before_apply"
                or active.get("index") != len(results) + 1
                or active.get("from") != next_step["from"]
                or active.get("to") != next_step["to"]):
            raise ValueError("resume refuses an in-flight or possibly applied edge")
    return prior, results


def execute(args, steps):
    if args.confirm != "FULL-MATRIX":
        raise ValueError("hardware execution requires --execute --confirm FULL-MATRIX")
    if getattr(args, "expected_serial", None) is None:
        raise ValueError("--expected-serial is required for hardware execution")
    if not args.output:
        raise ValueError("--output is required for hardware execution")
    if args.output.exists():
        raise ValueError("--output must name a new directory")
    controller_readbacks = getattr(args, "controller_readbacks", False)
    start_at = getattr(args, "start_at", 1)
    if type(start_at) is not int or not 1 <= start_at <= len(steps):
        raise ValueError(f"--start-at must be between 1 and {len(steps)}")
    try:
        args.output.mkdir(parents=True)
    except OSError as exc:
        raise ValueError(f"cannot create --output directory: {exc}") from exc
    runner = Runner(args.binary, args.output)
    summary = {"expected_serial": args.expected_serial, "port": args.port,
               "start": "split-right", "plan": steps, "results": [], "status": "running",
               "segment_start": start_at,
               "verification_mode": ("controller_readbacks_plus_independent_coregraphics"
                                     if controller_readbacks
                                     else "independent_wide_status_vcp_plus_coregraphics")}
    try:
        summary["binary_sha256"] = hashlib.sha256(args.binary.read_bytes()).hexdigest()
        resume_path = getattr(args, "resume", None)
        prior, prefix = validate_resume(resume_path, steps, args.port, args.expected_serial) if resume_path else (None, [])
        if prior and start_at > 1:
            raise ValueError("--start-at N>1 cannot be combined with --resume")
        if prior:
            summary["results"] = prefix.copy()
            summary["resume"] = {
                "origin": str(resume_path.resolve()),
                "prior_binary_sha256": prior.get("binary_sha256"),
                "prior_verification_mode": prior.get("verification_mode", "legacy_independent_status_vcp"),
                "interrupted": True,
                "warning": "Resuming a failed run; this matrix will not be reported as a clean pass.",
            }
            print("Warning: resuming an interrupted run; completion will remain marked interrupted.",
                  file=sys.stderr, flush=True)
        current = observe(runner, args.expected_serial, args.port)
        start_index = len(prefix) if prior else start_at - 1
        expected_start = steps[start_index]["from"]
        if role(current["state"], args.port) != expected_start:
            raise RuntimeError(f"initial monitor role is not {expected_start}")
        initial_mode = current["status"].get("local_mode") or {}
        initial_dimensions = (initial_mode.get("pixel_width"), initial_mode.get("pixel_height"))
        refresh = initial_mode.get("refresh_hz")
        if (not verify_resolution(current["status"], expected_start)[0]
                or not isinstance(refresh, (int, float)) or not math.isfinite(refresh) or refresh <= 0):
            raise RuntimeError(f"initial local mode does not match {expected_start}")
        if prior:
            last = prefix[-1]
            expected_mode = last.get("apply", {}).get("resolution", {}).get("after")
            if (signature(current["state"]) != last.get("after")
                    or (current["e8_applicable"] and current["e8"] != last.get("e8_after"))
                    or mode_signature(current["status"]) != mode_signature({"local_mode": expected_mode})):
                raise RuntimeError("fresh resume state differs from the last verified boundary")
        elif current["e8"] != expected_e8(expected_start, current["e8"]):
            raise RuntimeError(f"initial E8 does not match {expected_start} audio policy")
        summary["initial"] = {"state": current["state"], "e8": current["e8"],
                               "e8_applicable": current["e8_applicable"],
                               "display": current["display"], "local_mode": initial_mode}
        persist(args.output, summary)
        expected_audio = current["e8"]
        for step in steps[start_index:]:
            summary["active_edge"] = {"index": step["index"], "from": step["from"],
                                      "to": step["to"], "phase": "preflight"}
            persist(args.output, summary)
            if (args.output / "STOP").exists():
                raise RuntimeError("STOP file found; stopped between transitions")
            if controller_readbacks:
                passive = observe_mode(runner, args.expected_serial, args.port,
                                       f"modes-before-{step['index']:02d}")
                if mode_signature(passive["status"]) != mode_signature(current["status"]):
                    raise RuntimeError("passive pre-edge mode differs from the prior verified boundary")
                before = {**current, "display": passive["display"], "status": passive["status"]}
            else:
                before = observe(runner, args.expected_serial, args.port)
            if role(before["state"], args.port) != step["from"]:
                raise RuntimeError(f"expected {step['from']} before transition {step['index']}")
            if signature(before["state"]) != signature(current["state"]):
                raise RuntimeError("fresh pre-state differs from the prior verified boundary")
            if (before["e8_applicable"] != current["e8_applicable"]
                    or (before["e8_applicable"] and
                        (before["e8"] != current["e8"] or before["e8"] != expected_audio))
                    or mode_signature(before["status"]) != mode_signature(current["status"])):
                raise RuntimeError("fresh applicable audio or local mode differs from the prior verified boundary")
            if before["e8"] != expected_e8(step["from"], expected_audio):
                raise RuntimeError("pre-edge E8 does not match the current layout policy")
            before_resolution_ok, _ = verify_resolution(before["status"], step["from"])
            if not before_resolution_ok:
                raise RuntimeError("pre-edge local dimensions do not match the current layout role")
            expected = expected_steps(before["state"], step["to"], args.port)
            if len(expected) > 4 or any(name not in STEP_VALUES for name in expected):
                raise RuntimeError("expected plan exceeds the allowed step budget; no apply started")
            edge = {"index": step["index"], "from": step["from"], "to": step["to"],
                    "before": signature(before["state"]), "e8_before": before["e8"],
                    "e8_applicable_before": before["e8_applicable"],
                    "expected_steps": expected, "status": "running",
                    "controller_sha256": summary["binary_sha256"]}
            summary["active_edge"].update(edge)
            persist(args.output, summary)
            print(f"Applying edge {step['index']}/25: {step['from']} → {step['to']}", flush=True)
            try:
                if (args.output / "STOP").exists():
                    raise RuntimeError("STOP file found after preflight; no apply command started")
                registry = str(before["display"]["registry_id"])
                log = args.output / f"apply-{step['index']:02d}.jsonl"
                summary["active_edge"]["phase"] = "apply_started"
                persist(args.output, summary)
                apply_started = time.monotonic()
                try:
                    result = runner.call(f"apply-{step['index']:02d}", [
                        "--display", registry, "advanced", "apply", step["to"], "--port",
                        source_value(args.port), "--confirm", "--log", str(log),
                    ])
                finally:
                    edge["apply_elapsed_s"] = round(time.monotonic() - apply_started, 3)
                edge["apply"] = result
                if result.get("ok") is not True:
                    raise RuntimeError("apply returned ok=false or omitted ok")
                before_plan = result.get("plan", {}).get("before", {})
                if signature(before_plan) != signature(before["state"]):
                    raise RuntimeError("apply plan.before differs from fresh pre-state")
                if result.get("plan", {}).get("steps") != expected:
                    raise RuntimeError("apply plan step order differs from typed expected plan")
                if len(expected) > 4 or any(name not in STEP_VALUES for name in expected):
                    raise RuntimeError("expected plan exceeds the allowed step budget")
                got_steps = [item.get("step") for item in result.get("steps", [])]
                if got_steps != expected or len(got_steps) != len(set(got_steps)):
                    raise RuntimeError("apply step order differs from typed expected plan")
                if any(item.get("verified") is not True for item in result.get("steps", [])):
                    raise RuntimeError("an apply step did not verify")
                if any(item.get("write", {}).get("write_status") != 0
                       for item in result.get("steps", [])):
                    raise RuntimeError("an apply write returned a nonzero status")
                for item, name in zip(result.get("steps", []), expected):
                    code, value = STEP_VALUES[name]
                    if item.get("write", {}).get("request_hex", "").lower() != set_packet(code, value):
                        raise RuntimeError("apply result contains a write packet outside the typed plan")
                if result.get("recoveries"):
                    raise RuntimeError("apply reported recovery; stopping matrix")
                audio = result.get("audio")
                desired_audio = None if step["to"] == "fullscreen" else expected_e8(step["to"], before["e8"])
                if desired_audio is None:
                    if audio is not None:
                        raise RuntimeError("fullscreen unexpectedly included an audio operation")
                else:
                    if not isinstance(audio, dict) or audio.get("ok") is not True:
                        raise RuntimeError("audio verification evidence is missing or failed")
                    preflight = audio.get("preflight") or {}
                    if (preflight.get("status") != "ok" or preflight.get("max") != 2
                            or preflight.get("current") not in (0, 1)):
                        raise RuntimeError("audio preflight did not match the supported pair")
                audio_writes = (audio.get("setting_writes", 0)
                                if isinstance(audio, dict) else 0)
                if not isinstance(audio_writes, int) or isinstance(audio_writes, bool):
                    raise RuntimeError("apply audio write count is malformed")
                if audio_writes > 1:
                    raise RuntimeError("apply reported more than one audio setting write")
                if audio_writes != int(isinstance(audio, dict) and audio.get("write") is not None):
                    raise RuntimeError("apply audio write count differs from its write evidence")
                if desired_audio is not None:
                    if audio_writes != int(preflight["current"] != desired_audio):
                        raise RuntimeError("audio setting was omitted or repeated unnecessarily")
                    if audio_writes:
                        if (audio["write"].get("request_hex") != set_packet(0xE8, desired_audio)
                                or audio["write"].get("write_status") != 0):
                            raise RuntimeError("audio write is outside the approved E8 policy")
                        readback = audio.get("readback") or {}
                        if readback.get("status") != "ok" or readback.get("current") != desired_audio:
                            raise RuntimeError("audio write lacks verified readback")
                edge["audio_writes"] = audio_writes
                if not log.is_file():
                    raise RuntimeError("apply did not persist its requested JSONL journal")
                journal_rows = [json.loads(line) for line in log.read_text().splitlines() if line.strip()]
                journal_results = sum(row.get("phase") == "result" for row in journal_rows)
                if journal_results != len(expected):
                    raise RuntimeError("apply journal result count differs from planned steps")
                intents = [row for row in journal_rows if row.get("phase") == "intent"]
                writes = [row for row in journal_rows if row.get("phase") == "write_return"]
                commands = [tuple(row.get("command", [])) for row in intents]
                allowed_commands = [STEP_VALUES[name] for name in expected]
                if commands != allowed_commands or len(writes) != len(expected):
                    raise RuntimeError("apply journal writes differ from the exact typed step plan")
                for row, name in zip(writes, expected):
                    code, value = STEP_VALUES[name]
                    exchange = row.get("exchange", {})
                    if (exchange.get("request_hex", "").lower() != set_packet(code, value)
                            or exchange.get("write_status") != 0):
                        raise RuntimeError("apply journal contains a write outside the typed plan")
                if len(writes) > 4:
                    raise RuntimeError("apply exceeded the four video-write edge budget")
                if step["from"] == step["to"]:
                    if (got_steps or audio_writes
                            or result.get("resolution", {}).get("changed") is not False):
                        raise RuntimeError("self-transition apply changed video/audio or resolution")
                if controller_readbacks:
                    after_state = result.get("after")
                    if not isinstance(after_state, dict):
                        raise RuntimeError("apply result omitted verified monitor state")
                    enabled = after_state.get("enabled") is True
                    audio = result.get("audio")
                    if enabled:
                        if not isinstance(audio, dict):
                            raise RuntimeError("active layout omitted apply audio evidence")
                        count = audio.get("setting_writes")
                        audio_record = audio.get("readback") if count else audio.get("preflight")
                        after_e8 = audio_record_value(audio_record)
                    else:
                        after_e8 = None
                    passive = observe_mode(runner, args.expected_serial, args.port,
                                           f"modes-after-{step['index']:02d}")
                    after = {"state": after_state, "e2": after_state.get("raw_e2"),
                             "e3": state_e3(after_state), "e8": after_e8,
                             "e8_applicable": enabled, "display": passive["display"],
                             "status": passive["status"]}
                    edge["readback_provenance"] = {
                        "pre_state": "carried_from_previous_verified_boundary",
                        "post_state": "native_apply_result.after",
                        "audio": ("not_applicable_fullscreen" if not enabled else
                                  "native_apply_result.audio.readback" if audio.get("setting_writes") else
                                  "native_apply_result.audio.preflight"),
                        "local_mode": "fresh_passive_wide_advanced_modes",
                    }
                else:
                    after = observe(runner, args.expected_serial, args.port)
                    edge["readback_provenance"] = {
                        "pre_state": "fresh_wide_status", "post_state": "fresh_wide_status",
                        "audio": "fresh_wide_ddc_read", "local_mode": "fresh_wide_status",
                    }
                edge["after"] = signature(after["state"])
                edge["e8_after"] = after["e8"]
                edge["e8_applicable_after"] = after["e8_applicable"]
                edge["audio_readback"] = {"applicable_before": before["e8_applicable"],
                                           "value_before": before["e8"],
                                           "applicable_after": after["e8_applicable"],
                                           "value_after": after["e8"]}
                edge["audible_verification"] = "not_observed"
                edge["role_verified"] = role(after["state"], args.port) == step["to"]
                target_e8 = expected_e8(step["to"], before["e8"])
                if target_e8 is None:
                    edge["e8_verified"] = None
                    edge["e8_status"] = "not_applicable_fullscreen"
                    e8_ok = not after["e8_applicable"]
                else:
                    edge["e8_verified"] = after["e8_applicable"] and after["e8"] == target_e8
                    edge["e8_status"] = "verified" if edge["e8_verified"] else "mismatch"
                    e8_ok = edge["e8_verified"]
                edge["resolution_verified"], edge["resolution"] = verify_resolution(after["status"], step["to"])
                if (after["e2"] ^ before["e2"]) & ~0x0F:
                    raise RuntimeError("E2 changed outside the verified enable/mode bits")
                applied_mode = result.get("resolution", {}).get("after")
                observed_mode = after["status"].get("local_mode")
                if not isinstance(applied_mode, dict) or not isinstance(observed_mode, dict):
                    raise RuntimeError("resolution result or post-exit local mode is missing")
                for key in ("id", "pixel_width", "pixel_height", "refresh_hz"):
                    if applied_mode.get(key) != observed_mode.get(key):
                        raise RuntimeError("post-exit mode differs from apply resolution readback")
                if step["from"] == step["to"]:
                    edge["resolution_changed"] = (before["status"].get("local_mode") !=
                                                    after["status"].get("local_mode"))
                    edge["resolution_verified"] = edge["resolution_verified"] and not edge["resolution_changed"]
                if not (edge["role_verified"] and e8_ok and edge["resolution_verified"]):
                    raise RuntimeError("fresh readback did not match target role, E8, and resolution")
                edge["status"] = "verified"
                current = after
                expected_audio = after["e8"]
                summary["results"].append(edge)
                summary.pop("active_edge", None)
                print(f"Verified edge {step['index']}/25: {step['to']}; "
                      f"video writes={len(expected)}, audio writes={edge['audio_writes']}", flush=True)
            except Exception as exc:
                edge["error"] = str(exc)
                summary["status"] = "failed"
                summary["error"] = str(exc)
                summary["active_edge"]["phase"] = (
                    "failed_unknown_outcome" if "apply_elapsed_s" in edge else "failed_before_apply"
                )
                summary["active_edge"]["error"] = str(exc)
                persist(args.output, summary)
                print(f"Stopped at edge {step['index']}/25: {exc}", file=sys.stderr, flush=True)
                return 1
            persist(args.output, summary)
        if prior:
            summary["status"] = "complete_with_interruption"
        elif start_at > 1:
            summary["status"] = "complete_segment"
        else:
            summary["status"] = "complete"
        if prior:
            summary["complete_with_interruption"] = True
        summary["verified_count"] = len(summary["results"])
        summary["full_matrix_complete"] = start_at == 1 and len(summary["results"]) == len(steps)
        if start_at > 1:
            summary["covered_indexes"] = [edge["index"] for edge in summary["results"]]
        persist(args.output, summary)
        return 0
    except Exception as exc:
        summary["status"] = "failed"
        summary["error"] = str(exc)
        if summary.get("active_edge", {}).get("phase") == "preflight":
            summary["active_edge"]["phase"] = "failed_before_apply"
            summary["active_edge"]["error"] = str(exc)
        elif summary.get("active_edge", {}).get("phase") == "apply_started":
            summary["active_edge"]["phase"] = "failed_unknown_outcome"
            summary["active_edge"]["error"] = str(exc)
        persist(args.output, summary)
        return 1


def main(argv=None):
    project = Path(__file__).resolve().parents[1]
    args = parser_for(project).parse_args(argv)
    try:
        if args.execute:
            return execute(args, plan(args))
        if args.resume is not None:
            raise ValueError("--resume requires --execute")
        if args.confirm is not None:
            raise ValueError("--confirm is meaningful only with --execute")
        steps = plan(args)
        if type(args.start_at) is not int or not 1 <= args.start_at <= len(steps):
            raise ValueError(f"--start-at must be between 1 and {len(steps)}")
    except (ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    print(json.dumps({"dry_run": True, "start": args.start, "end": args.start,
                      "transition_count": len(steps), "max_video_writes_per_edge": 4,
                      "max_audio_writes_per_edge": 1, "segment_start": args.start_at,
                      "execution_steps": steps[args.start_at - 1:], "steps": steps}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
