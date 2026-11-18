"""Verify the native jev-use tasks against a repository harness (RFC #4268).

``--harness`` selects the AppKit (macOS), WPF or WinUI3 (Windows), or GTK3
(Linux) harness, or the cross-platform visual-only ``canvas`` fixture, which has no
accessibility tree and proves the OmniParser fallback (it needs the
cua-perception extension). The canvas publishes its state to a loopback
journal that this script serves; each published state is written to the task
state file unchanged except for the schema name. For every task and language this script launches a fresh harness in
task mode with its own state file (``CUA_<HARNESS>_TASK_STATE``), runs the
native runner against that exact process, then reads the harness state file
itself and checks it against the task's expected end state. The runners' own
outcome events are not the oracle. Evidence (redacted JSONL logs plus
``summary.json``) goes to a new ``--output-dir``. ``--capture-dir`` also
records sanitized ``get_window_state`` fixtures before and after each run.
``--density 12`` or ``24`` launches the AppKit, WPF, WinUI3, or GTK3 harness with
``CUA_<HARNESS>_TASK_DENSITY`` set, which adds benign distractor controls
before the task controls; the harness confirms the density in its state file
(#4312). ``measure_native.py`` turns the runner logs into an accuracy table.

Build the harness first, from the repository root:
``libs/cua-driver/tests/fixtures/build/macos.sh --only appkit``,
``libs/cua-driver/tests/fixtures/build/windows.ps1 -Targets wpf`` (or
``-Targets winui3``), or
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
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Callable, NamedTuple

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
    journal: bool = False  # the app posts its state to a loopback journal
    max_depth: int | None = None  # the task scope's walk depth, for --capture-dir
    density_env: str | None = None  # opt-in distractor density (#4312)



# Mirrors python/native_tasks.py HARNESSES; kept independent so the verifier
# does not trust the code it verifies.
HARNESSES = {
    "appkit": Harness(
        TEST_APPS / "harness-appkit/CuaTestHarness.AppKit.app", "Contents/MacOS/CuaTestHarness.AppKit",
        "CuaTestHarness AppKit", "cua.appkit_task_state_v1", "CUA_APPKIT_TASK_STATE",
        density_env="CUA_APPKIT_TASK_DENSITY",
    ),
    "wpf": Harness(
        TEST_APPS / "harness-wpf/CuaTestHarness.Wpf.exe", "",
        "CuaTestHarness WPF Tasks", "cua.wpf_task_state_v1", "CUA_WPF_TASK_STATE",
        density_env="CUA_WPF_TASK_DENSITY",
    ),
    "winui3": Harness(
        TEST_APPS / "harness-winui3/CuaTestHarness.WinUI3.exe", "",
        "CuaTestHarness WinUI3 Tasks", "cua.winui3_task_state_v1", "CUA_WINUI3_TASK_STATE",
        density_env="CUA_WINUI3_TASK_DENSITY",
    ),
    "gtk3": Harness(
        TEST_APPS / "harness-gtk3/CuaTestHarness.Gtk3", "",
        "CuaTestHarness GTK3 Tasks", "cua.gtk3_task_state_v1", "CUA_GTK3_TASK_STATE",
        density_env="CUA_GTK3_TASK_DENSITY",
    ),
    "canvas": Harness(
        REPO / "libs/cua-driver/tests/fixtures/apps/cross-platform/visual-only-canvas/main.py", "",
        "Cua Visual-Only Canvas Fixture", "cua.visual_canvas_task_state_v1", "CUA_CANVAS_TASK_STATE",
        journal=True, max_depth=1,
    ),
}
FORM_KINDS = ("choose-size", "counter", "save-note")
DENSITIES = (12, 24)
HARNESS_KINDS = {name: (("cancel",) if name == "canvas" else FORM_KINDS) for name in HARNESSES}

# The independent end-state check for each task kind, read from the app's own file.
EXPECTED = {
    "counter": lambda state: state.get("counter") == 3,
    "save-note": lambda state: state.get("note_saved") == NOTE_TEXT,
    "choose-size": lambda state: state.get("size") == "large" and state.get("agreed") is True,
    "cancel": lambda state: state.get("selected") == "cancel" and state.get("action_count") == 1,
}
# The executable candidates the deterministic mock provider must have acted on.
MOCK_ACTIONS = {
    "counter": ["ax:button:increment"] * 3,
    "save-note": ["ax:text_input:note:set:note", "ax:button:save-note"],
    "choose-size": ["ax:radio:large", "ax:checkbox:i-agree"],
    "cancel": ["visual:cancel"],
}


def runner_command(language: str) -> list[str]:
    if language == "python":
        return [sys.executable, "python/run_native.py"]
    return ["node", "--import", "tsx", "typescript/run_native.ts"]


def canvas_python() -> str:
    """A Tk-capable interpreter whose process ID is the fixture's own.

    A virtual environment's launcher on Windows starts the real interpreter as
    a child, so the fixture's published ``pid`` would not match the launched
    process; the base interpreter avoids that. ``CUA_CANVAS_PYTHON`` overrides.
    """
    return os.environ.get("CUA_CANVAS_PYTHON") or getattr(sys, "_base_executable", "") or sys.executable


def start_journal(harness: Harness, state: Path) -> tuple[str, Callable[[], None]]:
    """Serve a loopback journal that writes each published app state to ``state``."""

    class Journal(BaseHTTPRequestHandler):
        def log_message(self, *_args: object) -> None:
            pass

        def do_POST(self) -> None:  # noqa: N802 - http.server API
            length = int(self.headers.get("Content-Length") or 0)
            try:
                published = json.loads(self.rfile.read(min(length, 65536)))
                if not isinstance(published, dict):
                    raise ValueError("state must be an object")
            except ValueError:
                self.send_response(400)
                self.end_headers()
                return
            temporary = state.with_suffix(".tmp")
            temporary.write_text(json.dumps({**published, "schema": harness.state_schema}), encoding="utf-8")
            os.replace(temporary, state)
            self.send_response(204)
            self.end_headers()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Journal)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    def close() -> None:
        server.shutdown()
        server.server_close()

    return f"http://127.0.0.1:{server.server_address[1]}/journal", close


def launch_harness(
    harness: Harness, app: Path, state: Path, density: int | None = None
) -> tuple[subprocess.Popen, Callable[[], None]]:
    executable = app / harness.executable if harness.executable else app
    if not executable.is_file():
        raise SystemExit(f"harness is missing at {executable}; run the fixture build")
    close: Callable[[], None] = lambda: None
    if harness.journal:
        url, close = start_journal(harness, state)
        command = [canvas_python(), str(executable), "--journal-url", url]
    else:
        command = [str(executable)]
    env = {**os.environ, harness.state_env: str(state)}
    if harness.density_env:
        env.pop(harness.density_env, None)
    if density is not None:
        if harness.density_env is None:
            raise SystemExit("this harness has no distractor density mode")
        env[harness.density_env] = str(density)
    process = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_for_initial_state(process, state)
    except BaseException:
        close()
        raise
    return process, close


def wait_for_initial_state(process: subprocess.Popen, state: Path) -> None:
    for _ in range(100):
        if state.exists():
            try:
                initial = json.loads(state.read_text(encoding="utf-8"))
            except json.JSONDecodeError:
                initial = {}
            if initial.get("pid") == process.pid:
                return
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
         "--source", f"CuaTestHarness {harness_name} task mode ({label}), observed by get_window_state",
         *(["--max-depth", str(harness.max_depth)] if harness.max_depth is not None else [])],
        cwd=BASE, check=False, timeout=120,
    )
    if completed.returncode != 0:
        print(json.dumps({"event": "capture_failed", "label": label}), flush=True)


def verify(
    language: str, provider: str, harness_name: str, kind: str, app: Path, output: Path, work: Path,
    capture_dir: Path | None = None, density: int | None = None,
) -> dict:
    harness = HARNESSES[harness_name]
    task = f"{harness_name}-{kind}"
    suffix = f"-d{density}" if density is not None else ""
    state = work / f"{language}-{provider}-{task}{suffix}-state.json"
    log = output / f"{language}-{provider}-{task}{suffix}.jsonl"
    process, close_journal = launch_harness(harness, app, state, density)
    try:
        initial = read_state(harness, state, process.pid)
        if initial.get("density") != density:
            raise RuntimeError(f"harness reports density {initial.get('density')!r}, not {density!r}")
        if capture_dir is not None:
            capture(harness_name, process.pid, capture_dir / f"{language}-{task}{suffix}-initial.json", "initial")
        command = runner_command(language) + [
            "--task", task, "--provider", provider, "--pid", str(process.pid),
            "--state-file", str(state), "--note-text", NOTE_TEXT, "--log", str(log),
        ]
        if harness.journal:
            # A visual click may be refused in the background on X11; the canvas
            # task then offers an explicit foreground variant.
            command.append("--allow-foreground")
        completed = subprocess.run(command, cwd=BASE, check=False, timeout=300)
        observed = read_state(harness, state, process.pid)
        if capture_dir is not None:
            capture(harness_name, process.pid, capture_dir / f"{language}-{task}{suffix}-after.json", "after the task")
    finally:
        process.kill()
        process.wait(timeout=10)
        close_journal()
    events = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    result = {
        "language": language,
        "provider": provider,
        "task": task,
        "density": density,
        "distractor_actions": observed.get("distractor_actions"),
        "exit_code": completed.returncode,
        "runner_outcome": events[-1].get("outcome") if events else None,
        "initial": {key: initial.get(key) for key in ("counter", "agreed", "size", "note_saved", "selected", "action_count")},
        "observed": {key: observed.get(key) for key in ("counter", "agreed", "size", "note_saved", "selected", "action_count")},
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
    delivered = [action.removesuffix(":foreground") for action in result["actions"]]
    if provider == "mock" and delivered != MOCK_ACTIONS[kind]:
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
    parser.add_argument(
        "--density", type=int, choices=DENSITIES,
        help="add the harness's benign distractor controls (every harness except canvas)",
    )
    parser.add_argument("--output-dir", type=Path, required=True, help="new evidence directory")
    args = parser.parse_args()
    if args.live and not os.environ.get("TYPESAFE_API_KEY", "").strip():
        if not sys.stdin.isatty():
            raise SystemExit("Human prerequisite: provision TYPESAFE_API_KEY before a live run")
        os.environ["TYPESAFE_API_KEY"] = getpass.getpass("TypeSafe API key: ").strip()
    if args.density is not None and HARNESSES[args.harness].density_env is None:
        parser.error(f"--harness {args.harness} has no distractor density mode")
    unsupported = sorted(set(args.task or ()) - set(HARNESS_KINDS[args.harness]))
    if unsupported:
        parser.error(f"--harness {args.harness} has no task kind {', '.join(unsupported)}")
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
        "density": args.density,
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
                    for kind in args.task or HARNESS_KINDS[args.harness]:
                        result = verify(
                            language, provider, args.harness, kind, app, output, Path(work), capture_dir,
                            args.density,
                        )
                        summary["checks"].append(result)
                        print(json.dumps({"event": "independently_verified", **result}), flush=True)
        summary["complete"] = True
    finally:
        (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({"event": "native_verify_complete", "checks": len(summary["checks"])}), flush=True)


if __name__ == "__main__":
    main()
