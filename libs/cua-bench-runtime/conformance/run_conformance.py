"""Artifact-level conformance suite for any installed cdb executable."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TASK = ROOT / "conformance/tasks/synthetic-echo-v1/task.cuabench.json"
AGENTS = ROOT / "conformance/agents"
EXPECTED_CONFIG_DIGEST = (
    (ROOT / "conformance/golden/synthetic-echo-config-digest.txt")
    .read_text(encoding="utf-8")
    .strip()
)


def run(*arguments: str, expected: int) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(["cdb", *arguments], text=True, capture_output=True, check=False)
    if completed.returncode != expected:
        raise AssertionError(
            f"cdb {' '.join(arguments)} returned {completed.returncode}, expected {expected}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


def run_interrupt(directory: str) -> None:
    trial_id = "interrupted"
    command = [
        "cdb",
        "run",
        "--task",
        str(TASK),
        "--agent",
        str(AGENTS / "reference_hang.py"),
        "--out",
        directory,
        "--trial-id",
        trial_id,
        "--timeout",
        "20",
    ]
    creationflags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    process = subprocess.Popen(
        command,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=creationflags,
    )
    event_path = Path(directory) / trial_id / "events.ndjson"
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if event_path.is_file() and b'"process_spawned"' in event_path.read_bytes():
            break
        if process.poll() is not None:
            break
        time.sleep(0.05)
    if os.name == "nt":
        process.send_signal(signal.CTRL_BREAK_EVENT)
    else:
        process.terminate()
    stdout, stderr = process.communicate(timeout=10)
    if process.returncode != 5:
        raise AssertionError(
            f"interrupted trial returned {process.returncode}, expected 5\n"
            f"stdout:\n{stdout}\nstderr:\n{stderr}"
        )
    result = json.loads((Path(directory) / trial_id / "result.json").read_text())
    if not result["cleanup_ok"] or result["status"] != "interrupted":
        raise AssertionError(f"interrupted trial did not clean up: {result}")
    events = (Path(directory) / trial_id / "events.ndjson").read_text(encoding="utf-8")
    if '"type":"interrupted"' not in events:
        raise AssertionError("interrupted trial omitted the interruption event")


def main() -> int:
    run("validate", str(TASK), expected=0)
    with tempfile.TemporaryDirectory() as directory:
        run(
            "run",
            "--task",
            str(TASK),
            "--agent",
            str(AGENTS / "reference_ok.py"),
            "--out",
            directory,
            "--trial-id",
            "success",
            expected=0,
        )
        success_result = json.loads(
            (Path(directory) / "success/result.json").read_text(encoding="utf-8")
        )
        if success_result["config_digest"] != EXPECTED_CONFIG_DIGEST:
            raise AssertionError(
                "cross-platform config digest changed: "
                f"{success_result['config_digest']} != {EXPECTED_CONFIG_DIGEST}"
            )
        if success_result["participation"] != {
            "required": False,
            "status": "not_required",
            "passed": None,
            "observer": {"name": "none", "trust": "unavailable"},
            "requirements": [],
            "bindings": {
                "trial_id": "success",
                "task_digest": success_result["participation"]["bindings"]["task_digest"],
                "config_digest": success_result["config_digest"],
            },
            "receipt_digest": success_result["participation"]["receipt_digest"],
        }:
            raise AssertionError("headless task participation receipt changed")
        run("explain", str(Path(directory) / "success"), "--verify-only", expected=0)
        run(
            "run",
            "--task",
            str(TASK),
            "--agent",
            str(AGENTS / "reference_fail.py"),
            "--out",
            directory,
            "--trial-id",
            "graded-fail",
            expected=0,
        )
        result = json.loads((Path(directory) / "graded-fail/result.json").read_text())
        if result["evaluation"]["passed"] is not False:
            raise AssertionError("graded failure did not remain a result")
        run(
            "run",
            "--task",
            str(TASK),
            "--agent",
            str(AGENTS / "reference_hang.py"),
            "--out",
            directory,
            "--trial-id",
            "timeout",
            "--timeout",
            "0.2",
            expected=4,
        )
        run(
            "run",
            "--task",
            str(TASK),
            "--agent",
            str(AGENTS / "reference_ok.py"),
            "--out",
            directory,
            "--trial-id",
            "cleanup-fail",
            "--env",
            "local-fail-cleanup",
            expected=6,
        )
        run_interrupt(directory)
    print("Cua Driver Bench Runtime conformance passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
