"""pytest fixtures for cua-driver Python e2e tests.

Fixtures:
  binary           → str — path to cua-driver binary
  driver           → Driver (context manager, yields a started Driver)
  focus_monitor    → (proc, pid) — FocusMonitorApp running as frontmost sentinel
  ux_guard         → UXMonitor — started before the test, assert_clean() after
  html_server      → str — base URL of a local HTTP server serving assets/
  tauri_app        → (proc, pid, base_url) — repo-local Tauri harness
  electron_app     → (proc, pid, base_url) — repo-local Electron harness
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import threading
from http.server import HTTPServer, SimpleHTTPRequestHandler

import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
_TESTS_ROOT = os.path.dirname(_HERE)
_CUA_DRIVER_ROOT = os.path.dirname(_TESTS_ROOT)
_DRIVER_RS_ROOT = os.path.join(_CUA_DRIVER_ROOT, "rust")
_LIBS_ROOT = os.path.dirname(_CUA_DRIVER_ROOT)
_TEST_HARNESS_ROOT = os.path.join(_CUA_DRIVER_ROOT, "test-harness")

sys.path.insert(0, _HERE)
from driver_client import default_binary_path  # noqa: E402

_ASSETS_DIR = os.path.join(_HERE, "assets")
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"
_FOCUS_APP_DIR = os.path.join(_LIBS_ROOT, "cua-driver", "Tests", "FocusMonitorApp")
_FOCUS_APP_BUNDLE = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.app")
_FOCUS_APP_EXE = os.path.join(_FOCUS_APP_BUNDLE, "Contents", "MacOS", "FocusMonitorApp")

# Tauri is built from the repo-local shared harness rather than downloaded.
_TAURI_HARNESS_DIR = os.path.join(
    _TEST_HARNESS_ROOT, "apps", "cross-platform", "tauri"
)
_TAURI_BUILD_SCRIPT = os.path.join(_TAURI_HARNESS_DIR, "build.sh")
_TAURI_APP_BUNDLE = os.path.join(
    _DRIVER_RS_ROOT,
    "test-apps",
    "harness-tauri",
    "CuaTestHarness.Tauri.app",
)
_TAURI_APP_EXE = os.path.join(
    _TAURI_APP_BUNDLE, "Contents", "MacOS", "CuaTestHarness.Tauri"
)

# Electron is built from the repo-local shared harness rather than downloaded.
_ELECTRON_HARNESS_DIR = os.path.join(
    _TEST_HARNESS_ROOT, "apps", "cross-platform", "electron"
)
_ELECTRON_BUILD_SCRIPT = os.path.join(_ELECTRON_HARNESS_DIR, "build.sh")
_ELECTRON_APP_BUNDLE = os.path.join(
    _DRIVER_RS_ROOT,
    "test-apps",
    "harness-electron",
    "CuaTestHarness.Electron.app",
)
_ELECTRON_APP_EXE = os.path.join(
    _ELECTRON_APP_BUNDLE, "Contents", "MacOS", "Electron"
)


# ── binary ────────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def binary() -> str:
    return default_binary_path()


# ── driver ────────────────────────────────────────────────────────────────────

@pytest.fixture
def driver(binary):
    """Yield a started Driver instance."""
    sys.path.insert(0, _HERE)
    from harness.driver import Driver
    with Driver(binary) as d:
        yield d


# ── focus monitor ─────────────────────────────────────────────────────────────

def _build_focus_app() -> None:
    if not os.path.exists(_FOCUS_APP_EXE):
        build_sh = os.path.join(_FOCUS_APP_DIR, "build.sh")
        subprocess.run([build_sh], check=True)


def _launch_focus_app():
    """Launch FocusMonitorApp, return (proc, pid)."""
    _build_focus_app()
    proc = subprocess.Popen(
        [_FOCUS_APP_EXE],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    for _ in range(40):
        line = proc.stdout.readline().strip()
        if line.startswith("FOCUS_PID="):
            pid = int(line.split("=", 1)[1])
            return proc, pid
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError("FocusMonitorApp did not print FOCUS_PID in time")


def _wait_for_no_focus_app(timeout_s: float = 3.0) -> None:
    """Block until no FocusMonitorApp process is running."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        r = subprocess.run(
            ["pgrep", "-x", "FocusMonitorApp"],
            capture_output=True,
        )
        if r.returncode != 0:
            return
        time.sleep(0.1)


@pytest.fixture(scope="module")
def focus_monitor():
    """Launch FocusMonitorApp as frontmost sentinel for the test module."""
    # Kill any stale instances from previous modules and wait for full exit.
    subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
    _wait_for_no_focus_app()
    try:
        os.remove(_LOSS_FILE)
    except FileNotFoundError:
        pass

    proc, pid = _launch_focus_app()
    time.sleep(0.8)
    yield proc, pid

    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
    subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
    # Wait for the process to fully exit before the next module starts.
    _wait_for_no_focus_app()
    try:
        os.remove(_LOSS_FILE)
    except FileNotFoundError:
        pass


@pytest.fixture(autouse=False)
def activate_focus_monitor(focus_monitor):
    """Re-activate FocusMonitorApp before each test (by pid)."""
    _, pid = focus_monitor
    subprocess.run(
        ["osascript", "-e",
         f'tell application "System Events" to set frontmost of (first process whose unix id is {pid}) to true'],
        check=False,
    )
    time.sleep(0.4)


# ── UX guard ──────────────────────────────────────────────────────────────────

@pytest.fixture
def ux_guard(focus_monitor):
    """Start UXMonitor before the test, call assert_clean() after."""
    _, sentinel_pid = focus_monitor
    sys.path.insert(0, _HERE)
    from harness.monitor import UXMonitor
    mon = UXMonitor(sentinel_pid=sentinel_pid)
    mon.start()
    yield mon
    mon.stop()
    mon.assert_clean()


# ── local HTML server ─────────────────────────────────────────────────────────

class _SilentHandler(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


@pytest.fixture(scope="session")
def html_server():
    """Serve the assets/ directory over HTTP; return base URL."""
    handler = lambda *args, **kwargs: _SilentHandler(
        *args, directory=_ASSETS_DIR, **kwargs
    )
    server = HTTPServer(("127.0.0.1", 0), handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{port}"
    server.shutdown()


# ── Tauri app ─────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def tauri_app():
    """Build, launch, and yield (proc, pid, base_url) for the Tauri harness.

    This uses test-harness/apps/cross-platform/tauri, the same shared DOM
    harness used by the Electron/WebView/WKWebView harnesses. It does not
    download a prebuilt Tauri app release.
    """
    if not os.path.exists(_TAURI_APP_EXE):
        if not os.path.exists(_TAURI_BUILD_SCRIPT):
            pytest.skip("repo-local Tauri harness build script not found")
        try:
            subprocess.run([_TAURI_BUILD_SCRIPT], check=True)
        except (OSError, subprocess.CalledProcessError) as e:
            pytest.skip(f"Tauri harness build failed: {e}")

    if not os.path.exists(_TAURI_APP_EXE):
        pytest.skip("Tauri harness app not found after build")

    subprocess.run(["pkill", "-f", "CuaTestHarness.Tauri"], check=False)
    time.sleep(0.5)

    proc = subprocess.Popen(
        [_TAURI_APP_EXE],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    pid = proc.pid
    time.sleep(3.0)

    base_url = "file://shared-web-harness"
    yield proc, pid, base_url

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
    subprocess.run(["pkill", "-f", "CuaTestHarness.Tauri"], check=False)


# ── Electron app ──────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def electron_app():
    """Build, launch, and yield (proc, pid, base_url) for the Electron harness.

    This uses test-harness/apps/cross-platform/electron, the same shared DOM
    harness used by the Rust harness tests. It does not download a prebuilt
    Electron app release.
    """
    if not os.path.exists(_ELECTRON_APP_EXE):
        if not os.path.exists(_ELECTRON_BUILD_SCRIPT):
            pytest.skip("repo-local Electron harness build script not found")
        try:
            subprocess.run([_ELECTRON_BUILD_SCRIPT], check=True)
        except (OSError, subprocess.CalledProcessError) as e:
            pytest.skip(f"Electron harness build failed: {e}")

    if not os.path.exists(_ELECTRON_APP_EXE):
        pytest.skip("Electron harness app not found after build")

    subprocess.run(["pkill", "-f", "CuaTestHarness.Electron"], check=False)
    subprocess.run(["pkill", "-f", "Electron.app/Contents/MacOS/Electron"], check=False)
    time.sleep(0.5)

    proc = subprocess.Popen(
        [_ELECTRON_APP_EXE, "--force-renderer-accessibility"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(4.0)  # Electron takes a bit longer with AX enabled

    pid = proc.pid

    base_url = "file://shared-web-harness"
    yield proc, pid, base_url

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
    subprocess.run(["pkill", "-f", "CuaTestHarness.Electron"], check=False)
    subprocess.run(["pkill", "-f", "Electron.app/Contents/MacOS/Electron"], check=False)
