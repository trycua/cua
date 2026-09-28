"""Verify the native jev-use tasks against a repository harness (RFC #4268).

``--harness`` selects the AppKit (macOS), WPF (Windows), or GTK3 (Linux)
harness. For every task and language this script launches a fresh harness in
task mode with its own state file (``CUA_<HARNESS>_TASK_STATE``), runs the
native runner against that exact process, then reads the harness state file
itself and checks it against the task's expected end state. The runners' own
outcome events are not the oracle. Evidence (redacted JSONL logs plus
``summary.json``) goes to a new ``--output-dir``. ``--capture-dir`` also
records sanitized ``get_window_state`` fixtures before and after each run.

Build the harness first, from the repository root:
``libs/cua-driver/tests/fixtures/build/macos.sh --only appkit``,
``libs/cua-driver/tests/fixtures/build/windows.ps1 -Targets wpf``, or
``libs/cua-driver/tests/fixtures/build/linux.sh --only gtk3``.
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
from typing import NamedTuple

BASE = Path(__file__).resolve().parent
REPO = BASE.parents[3]
TEST_APPS = REPO / "libs/cua-driver/rust/test-apps"
NOTE_TEXT = "jev-use native note"


class Harness(NamedTuple):
    app: Path
    executable: str  # relative to ``app`` (empty when ``app`` is the executable)
    window_title: str
    state_schema: str
    state_env: str


# Mirrors python/native_tasks.py HARNESSES; kept independent so the verifier
# does not trust the code it verifies.
HARNESSES = {
    "appkit": Harness(
        TEST_APPS / "harness-appkit/CuaTestHarness.AppKit.app", "Contents/MacOS/CuaTestHarness.AppKit",
        "CuaTestHarness AppKit", "cua.appkit_task_state_v1", "CUA_APPKIT_TASK_STATE",
    ),
    "wpf": Harness(
        TEST_APPS / "harness-wpf/CuaTestHarness.Wpf.exe", "",
        "CuaTestHarness WPF Tasks", "cua.wpf_task_state_v1", "CUA_WPF_TASK_STATE",
    ),
    "gtk3": Harness(
        TEST_APPS / "harness-gtk3/CuaTestHarness.Gtk3", "",
        "CuaTestHarness GTK3 Tasks", "cua.gtk3_task_state_v1", "CUA_GTK3_TASK_STATE",
    ),
}

# The independent end-state check for each task kind, read from the app's own file.
EXPECTED = {
    "counter": lambda state: state.get("counter") == 3,
    "save-note": lambda state: state.get("note_saved") == NOTE_TEXT,
    "choose-size": lambda state: state.get("size") == "large" and state.get("agreed") is True,
}
# The executable candidates the deterministic mock provider must have acted on.
MOCK_ACTIONS = {
    "counter": ["ax:button:increment"] * 3,
    "save-note": ["ax:text_input:note:set:note", "ax:button:save-note"],
    "choose-size": ["ax:radio:large", "ax:checkbox:i-agree"],
}


def runner_command(language: str) -> list[str]:
    if language == "python":
        return [sys.executable, "python/run_native.py"]
    return ["node", "--import", "tsx", "typescript/run_native.ts"]


def launch_harness(harness: Harness, app: Path, state: Path) -> subprocess.Popen:
    executable = app / harness.executable if harness.executable else app
    if not executable.is_file():
        raise SystemExit(f"harness is missing at {executable}; run the fixture build")
    env = {**os.environ, harness.state_env: str(state)}
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
            raise RuntimeError("harness exited during launch")
        time.sleep(0.1)
    process.kill()
    raise RuntimeError("harness did not publish its initial task state")


def read_state(harness: Harness, state: Path, pid: int) -> dict:
    observed = json.loads(state.read_text(encoding="utf-8"))
    if observed.get("schema") != harness.state_schema or observed.get("pid") != pid:
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


def capture(harness_name: str, pid: int, output: Path, label: str) -> None:
    """Record one sanitized get_window_state fixture; a failure is not fatal."""
    harness = HARNESSES[harness_name]
    completed = subprocess.run(
        [sys.executable, "python/capture_window_state.py", "--pid", str(pid),
         "--title", harness.window_title, "--output", str(output),
         "--source", f"CuaTestHarness {harness_name} task mode ({label}), observed by get_window_state"],
        cwd=BASE, check=False, timeout=120,
    )
    if completed.returncode != 0:
        print(json.dumps({"event": "capture_failed", "label": label}), flush=True)


def verify(
    language: str, provider: str, harness_name: str, kind: str, app: Path, output: Path, work: Path,
    capture_dir: Path | None = None,
) -> dict:
    harness = HARNESSES[harness_name]
    task = f"{harness_name}-{kind}"
    state = work / f"{language}-{provider}-{task}-state.json"
    log = output / f"{language}-{provider}-{task}.jsonl"
    process = launch_harness(harness, app, state)
    try:
        initial = read_state(harness, state, process.pid)
        if capture_dir is not None:
            capture(harness_name, process.pid, capture_dir / f"{language}-{task}-initial.json", "initial")
        command = runner_command(language) + [
            "--task", task, "--provider", provider, "--pid", str(process.pid),
            "--state-file", str(state), "--note-text", NOTE_TEXT, "--log", str(log),
        ]
        completed = subprocess.run(command, cwd=BASE, check=False, timeout=300)
        observed = read_state(harness, state, process.pid)
        if capture_dir is not None:
            capture(harness_name, process.pid, capture_dir / f"{language}-{task}-after.json", "after the task")
    finally:
        process.kill()
        process.wait(timeout=10)
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
    result["verified"] = bool(EXPECTED[kind](observed))
    if result["observed"].get("note_saved") == NOTE_TEXT:
        # The note text is a secret task parameter; never keep it in evidence.
        result["observed"]["note_saved"] = "[note text]"
    if completed.returncode != 0 or result["runner_outcome"] != "verified":
        raise RuntimeError(f"runner did not report verified: {json.dumps(result)}")
    if not result["verified"]:
        raise RuntimeError(f"independent harness state does not match: {json.dumps(result)}")
    if provider == "mock" and result["actions"] != MOCK_ACTIONS[kind]:
        raise RuntimeError(f"mock provider took unexpected actions: {json.dumps(result)}")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--harness", choices=sorted(HARNESSES), default="appkit")
    parser.add_argument("--app", type=Path, help="harness app or executable (default: the fixture build output)")
    parser.add_argument("--typescript", action="store_true", help="also verify the TypeScript runner")
    parser.add_argument("--live", action="store_true", help="also verify live Jev; needs a TypeSafe key")
    parser.add_argument(
        "--s1", action="store_true",
        help="also verify Cua-S1 through the loopback decision service named by CUA_S1_DECISION_URL",
    )
    parser.add_argument("--task", action="append", choices=sorted(EXPECTED), help="limit to task kinds")
    parser.add_argument("--capture-dir", type=Path, help="also record sanitized window-state fixtures")
    parser.add_argument("--output-dir", type=Path, required=True, help="new evidence directory")
    args = parser.parse_args()
    if args.live and not os.environ.get("TYPESAFE_API_KEY", "").strip():
        if not sys.stdin.isatty():
            raise SystemExit("Human prerequisite: provision TYPESAFE_API_KEY before a live run")
        os.environ["TYPESAFE_API_KEY"] = getpass.getpass("TypeSafe API key: ").strip()
    if args.s1:
        sys.path.insert(0, str(BASE / "python"))
        from s1_service import S1ServiceError, s1_service_url

        try:
            s1_service_url()
        except S1ServiceError as error:
            raise SystemExit(f"Prerequisite: {error}; start the S1 decision service first") from None
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    app = args.app or HARNESSES[args.harness].app
    capture_dir = args.capture_dir.resolve() if args.capture_dir else None
    if capture_dir is not None:
        capture_dir.mkdir(parents=True, exist_ok=True)
    summary: dict = {
        "complete": False,
        "harness": args.harness,
        "live_requested": args.live,
        "s1_requested": args.s1,
        "typescript_requested": args.typescript,
        "checks": [],
    }
    try:
        with tempfile.TemporaryDirectory(prefix="jev-native-") as work:
            for language in ["python", "typescript"] if args.typescript else ["python"]:
                providers = ["mock"] + (["live"] if args.live else []) + (["s1"] if args.s1 else [])
                for provider in providers:
                    for kind in args.task or sorted(EXPECTED):
                        result = verify(
                            language, provider, args.harness, kind, app, output, Path(work), capture_dir
                        )
                        summary["checks"].append(result)
                        print(json.dumps({"event": "independently_verified", **result}), flush=True)
        summary["complete"] = True
    finally:
        (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({"event": "native_verify_complete", "checks": len(summary["checks"])}), flush=True)


if __name__ == "__main__":
    main()
