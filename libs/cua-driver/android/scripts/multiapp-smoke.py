#!/usr/bin/env python3
"""Qualify two installed apps, task switching, stale handles, and Stop cleanup.

Uses no app-data or UI-input oracle. Run on an explicitly selected disposable
device with neither app already running. Does not install or reset apps.
"""
import argparse
import base64
import json
import pathlib
import re
import subprocess
import time


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--device", required=True)
    p.add_argument("--driver", type=pathlib.Path, required=True)
    p.add_argument("--app", action="append", required=True)
    p.add_argument("--evidence-dir", type=pathlib.Path, required=True)
    args = p.parse_args()
    assert len(args.app) == 2 and len(set(args.app)) == 2
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    records = []
    sid = None

    def shell(*cmd):
        return subprocess.check_output(["adb", "-s", args.device, "shell", *cmd], text=True, timeout=15)

    def main_focus():
        state = shell("dumpsys", "window", "displays")
        # One focus per display; display 0 is first in WindowManager output.
        match = re.search(r"Display: mDisplayId=0\b(.*?)(?=\n\s*Display: mDisplayId=|\Z)", state, re.S)
        assert match, "Main display missing"
        focus = re.search(r"mCurrentFocus=(.+)", match[1])
        assert focus, "Main-display focus missing"
        return focus[1]

    def call(*cmd, expected=0):
        command = [str(args.driver), "--device", args.device]
        if sid:
            command += ["--session", sid]
        r = subprocess.run(command + list(map(str, cmd)), capture_output=True, text=True, timeout=15)
        value = json.loads(r.stdout)
        assert r.returncode == expected and value["exit_code"] == expected, value
        image = value.get("data", {}).pop("image_base64", None)
        if image:
            (args.evidence_dir / f"frame-{len(records):02d}.png").write_bytes(base64.b64decode(image))
        records.append(value)
        return value

    baseline = main_focus()
    created = call("session", "create", "--allow-app", args.app[0], "--allow-app", args.app[1],
                   "--size", "540x960", "--density", "160")["data"]
    sid = created["session_id"]
    tasks = []
    try:
        first = call("app", "launch", "--package", args.app[0])["data"]
        tasks.append(first["task_id"])
        time.sleep(1)
        old = call("snapshot", "--target", first["target_id"])["data"]
        # A static application must produce a genuine new frame after 5s.
        time.sleep(6)
        fresh = call("snapshot", "--target", first["target_id"])["data"]
        assert fresh["frame_age_ms"] <= 5000 and fresh["frame_time_source"] == "producer_monotonic_ns"
        stale = call("tap", "--snapshot", old["snapshot_id"], "--x", 0, "--y", 0, expected=3)
        assert stale["error"]["reason"] == "stale_snapshot"
        second = call("app", "launch", "--package", args.app[1])["data"]
        tasks.append(second["task_id"])
        assert second["display_id"] == first["display_id"] == created["display_id"] != 0
        assert second["owned_task_count"] == 2
        refused = call("snapshot", "--target", first["target_id"], expected=3)
        assert refused["error"]["reason"] == "stale_target"
        for expected in (first, second, first):
            switched = call("app", "launch", "--package", expected["package"])["data"]
            assert switched["task_id"] == expected["task_id"] and switched["display_id"] == created["display_id"]
            assert switched["target_id"] != expected["target_id"] and switched["owned_task_count"] == 2
            call("snapshot", "--target", switched["target_id"])
            assert main_focus() == baseline, "Main-display focus changed"
        (args.evidence_dir / "tasks-active.txt").write_text(shell("am", "stack", "list"))
    finally:
        call("session", "stop")
        (args.evidence_dir / "responses.json").write_text(json.dumps(records, indent=2))
    stack = shell("am", "stack", "list")
    assert all(not re.search(r"taskId=" + str(task) + r":", stack) for task in tasks)
    assert main_focus() == baseline
    (args.evidence_dir / "tasks-stopped.txt").write_text(stack)
    print("PASS: static capture, stale handles, repeated owned-task switching, main focus, and two-task cleanup")


if __name__ == "__main__":
    main()
