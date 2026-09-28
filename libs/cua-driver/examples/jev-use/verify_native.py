"""Verify the native jev-use tasks against the macOS AppKit harness (RFC #4268).

For every task and language this script launches a fresh AppKit harness with
its own ``CUA_APPKIT_TASK_STATE`` file, runs the native runner against that
exact process, then reads the harness state file itself and checks it against
the task's expected end state. The runners' own outcome events are not the
oracle. Evidence (redacted JSONL logs plus ``summary.json``) goes to a new
``--output-dir``.

Build the harness first:
``libs/cua-driver/tests/fixtures/build/macos.sh --only appkit``.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

BASE = Path(__file__).resolve().parent
REPO = BASE.parents[3]
DEFAULT_APP = REPO / "libs/cua-driver/rust/test-apps/harness-appkit/CuaTestHarness.AppKit.app"
STATE_SCHEMA = "cua.appkit_task_state_v1"
NOTE_TEXT = "jev-use native note"

# The independent end-state check for each task, read from the app's own file.
EXPECTED = {
    "appkit-counter": lambda state: state.get("counter") == 3,
    "appkit-save-note": lambda state: state.get("note_saved") == NOTE_TEXT,
    "appkit-choose-size": lambda state: state.get("size") == "large" and state.get("agreed") is True,
}
# The executable candidates the deterministic mock provider must have acted on.
MOCK_ACTIONS = {
    "appkit-counter": ["ax:button:increment"] * 3,
    "appkit-save-note": ["ax:text_input:note:set:note", "ax:button:save-note"],
    "appkit-choose-size": ["ax:radio:large", "ax:checkbox:i-agree"],
}


def runner_command(language: str) -> list[str]:
    if language == "python":
        return [sys.executable, "python/run_native.py"]
    return ["node", "--import", "tsx", "typescript/run_native.ts"]


def launch_harness(app: Path, state: Path) -> subprocess.Popen:
    executable = app / "Contents/MacOS/CuaTestHarness.AppKit"
    if not executable.is_file():
        raise SystemExit(f"AppKit harness is missing at {executable}; run the fixture build")
    env = {**os.environ, "CUA_APPKIT_TASK_STATE": str(state)}
    process = subprocess.Popen(
        [str(executable)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    for _ in range(100):
        if state.exists():
            try:
                initial = json.loads(state.read_text(encoding="utf-8"))
            except json.JSONDecodeError:
                initial = {}
            if initial.get("pid") == process.pid:
                return process
        if process.poll() is not None:
            raise RuntimeError("AppKit harness exited during launch")
        time.sleep(0.1)
    process.kill()
    raise RuntimeError("AppKit harness did not publish its initial task state")


def read_state(state: Path, pid: int) -> dict:
    observed = json.loads(state.read_text(encoding="utf-8"))
    if observed.get("schema") != STATE_SCHEMA or observed.get("pid") != pid:
        raise RuntimeError("harness state file does not belong to the launched harness")
    return observed


def summarize(events: list[dict]) -> dict:
    steps = [event for event in events if event.get("event") == "step"]
    acted = [
        event for event in steps
        if event.get("tool") and not event.get("action_error")
    ]
    return {
        "actions": [event["candidate"] for event in acted],
        "tools": sorted({event["tool"] for event in acted}),
        "delivery_modes": sorted({event.get("delivery_mode") or "none" for event in acted}),
        "schemas": sorted({event.get("schema") for event in steps if event.get("schema")}),
        "sources": sorted({event.get("source") for event in acted if event.get("source")}),
        "risk_excluded": sorted({key for event in steps for key in event.get("compose", {}).get("risk_excluded", {})}),
        "max_candidates": max((event.get("candidate_count", 0) for event in steps), default=0),
        "visual_statuses": sorted({event.get("visual", {}).get("status") for event in steps}),
        "reobserved": any(event.get("observation", {}).get("reobserved") for event in steps),
    }


def verify(language: str, provider: str, task: str, app: Path, output: Path, work: Path) -> dict:
    state = work / f"{language}-{provider}-{task}-state.json"
    log = output / f"{language}-{provider}-{task}.jsonl"
    harness = launch_harness(app, state)
    try:
        initial = read_state(state, harness.pid)
        command = runner_command(language) + [
            "--task", task, "--provider", provider, "--pid", str(harness.pid),
            "--state-file", str(state), "--note-text", NOTE_TEXT, "--log", str(log),
        ]
        completed = subprocess.run(command, cwd=BASE, check=False, timeout=300)
        observed = read_state(state, harness.pid)
    finally:
        harness.kill()
        harness.wait(timeout=10)
    events = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    result = {
        "language": language,
        "provider": provider,
        "task": task,
        "exit_code": completed.returncode,
        "runner_outcome": events[-1].get("outcome") if events else None,
        "initial": {key: initial.get(key) for key in ("counter", "agreed", "size", "note_saved")},
        "observed": {key: observed.get(key) for key in ("counter", "agreed", "size", "note_saved")},
        **summarize(events),
    }
    result["verified"] = bool(EXPECTED[task](observed))
    if result["observed"].get("note_saved") == NOTE_TEXT:
        # The note text is a secret task parameter; never keep it in evidence.
        result["observed"]["note_saved"] = "[note text]"
    if completed.returncode != 0 or result["runner_outcome"] != "verified":
        raise RuntimeError(f"runner did not report verified: {json.dumps(result)}")
    if not result["verified"]:
        raise RuntimeError(f"independent harness state does not match: {json.dumps(result)}")
    if provider == "mock" and result["actions"] != MOCK_ACTIONS[task]:
        raise RuntimeError(f"mock provider took unexpected actions: {json.dumps(result)}")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=DEFAULT_APP, help="CuaTestHarness.AppKit.app")
    parser.add_argument("--typescript", action="store_true", help="also verify the TypeScript runner")
    parser.add_argument("--live", action="store_true", help="also verify live Jev; needs a TypeSafe key")
    parser.add_argument("--task", action="append", choices=sorted(EXPECTED), help="limit to tasks")
    parser.add_argument("--output-dir", type=Path, required=True, help="new evidence directory")
    args = parser.parse_args()
    if args.live and not os.environ.get("TYPESAFE_API_KEY", "").strip():
        if not sys.stdin.isatty():
            raise SystemExit("Human prerequisite: provision TYPESAFE_API_KEY before a live run")
        os.environ["TYPESAFE_API_KEY"] = getpass.getpass("TypeSafe API key: ").strip()
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    summary: dict = {
        "complete": False,
        "harness": "appkit",
        "live_requested": args.live,
        "typescript_requested": args.typescript,
        "checks": [],
    }
    try:
        with tempfile.TemporaryDirectory(prefix="jev-native-") as work:
            for language in ["python", "typescript"] if args.typescript else ["python"]:
                for provider in ["mock", "live"] if args.live else ["mock"]:
                    for task in args.task or sorted(EXPECTED):
                        result = verify(language, provider, task, args.app, output, Path(work))
                        summary["checks"].append(result)
                        print(json.dumps({"event": "independently_verified", **result}), flush=True)
        summary["complete"] = True
    finally:
        (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({"event": "native_verify_complete", "checks": len(summary["checks"])}), flush=True)


if __name__ == "__main__":
    main()
