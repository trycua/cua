"""pytest fixtures for cua-driver integration tests (v2 harness).

Fixtures:
  binary           → str — path to cua-driver binary
  driver           → Driver (context manager, yields a started Driver)
  focus_monitor    → (proc, pid) — FocusMonitorApp running as frontmost sentinel
  activate_focus_monitor → autouse=False; re-activates sentinel before each test
  ux_guard         → UXMonitor — started before the test, assert_clean() after
  html_server      → str — base URL of a local HTTP server serving assets/
  tauri_app        → (proc, pid, base_url) — Tauri desktop-test-app
  electron_app     → (proc, pid, base_url) — Electron desktop-test-app
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import threading
from http.server import HTTPServer, SimpleHTTPRequestHandler
import urllib.request

import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
_TESTS_DIR = os.path.dirname(_HERE)                        # Tests/
_DRIVER_ROOT = os.path.dirname(_TESTS_DIR)                 # libs/cua-driver/

sys.path.insert(0, _HERE)
sys.path.insert(0, os.path.join(_HERE, "harness"))

from driver_client import default_binary_path  # noqa: E402

_ASSETS_DIR = os.path.join(_HERE, "assets")
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"
_FOCUS_APP_DIR = os.path.join(_TESTS_DIR, "FocusMonitorApp")
_FOCUS_APP_BUNDLE = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.app")
_FOCUS_APP_EXE = os.path.join(_FOCUS_APP_BUNDLE, "Contents", "MacOS", "FocusMonitorApp")

# Tauri / Electron desktop test app config
_TAURI_VERSION = "v0.2.2"
_TAURI_BINARY_NAME = "desktop-test-app-macos-arm64"
_TAURI_DOWNLOAD_URL = (
    f"https://github.com/trycua/desktop-test-app/releases/download/"
    f"{_TAURI_VERSION}/{_TAURI_BINARY_NAME}"
)
_TAURI_APP_DIR = os.path.join(_ASSETS_DIR, "tauri")
_TAURI_BINARY_PATH = os.path.join(_TAURI_APP_DIR, _TAURI_BINARY_NAME)

_ELECTRON_VERSION = "v0.1.0"
_ELECTRON_APP_NAME = "desktop-test-app-electron.app"
_ELECTRON_DOWNLOAD_URL = (
    f"https://github.com/trycua/desktop-test-app-electron/releases/download/"
    f"{_ELECTRON_VERSION}/desktop-test-app-electron-{_ELECTRON_VERSION[1:]}-arm64-mac.zip"
)
_ELECTRON_APP_DIR = os.path.join(_ASSETS_DIR, "electron")


# ── binary ────────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def binary() -> str:
    return default_binary_path()


# ── driver ────────────────────────────────────────────────────────────────────

@pytest.fixture
def driver(binary):
    """Yield a started Driver instance."""
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
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        r = subprocess.run(["pgrep", "-x", "FocusMonitorApp"], capture_output=True)
        if r.returncode != 0:
            return
        time.sleep(0.1)


@pytest.fixture(scope="module")
def focus_monitor():
    """Launch FocusMonitorApp as frontmost sentinel for the test module."""
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
         f'tell application "System Events" to set frontmost of '
         f'(first process whose unix id is {pid}) to true'],
        check=False,
    )
    time.sleep(0.4)


# ── UX guard ──────────────────────────────────────────────────────────────────

@pytest.fixture
def ux_guard(focus_monitor):
    """Start UXMonitor before the test, call assert_clean() after."""
    _, sentinel_pid = focus_monitor
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


# ── download helpers ──────────────────────────────────────────────────────────

def _download_and_extract(url: str, dest_dir: str, marker_path: str) -> None:
    if os.path.exists(marker_path):
        return
    os.makedirs(dest_dir, exist_ok=True)
    zip_path = os.path.join(dest_dir, "_download.zip")
    print(f"\n  Downloading {url} …")
    urllib.request.urlretrieve(url, zip_path)
    subprocess.run(["unzip", "-q", "-o", zip_path, "-d", dest_dir], check=True)
    subprocess.run(["xattr", "-cr", dest_dir], check=False)
    os.remove(zip_path)


def _download_binary(url: str, dest_path: str) -> None:
    if os.path.exists(dest_path):
        return
    os.makedirs(os.path.dirname(dest_path), exist_ok=True)
    print(f"\n  Downloading {url} …")
    urllib.request.urlretrieve(url, dest_path)
    os.chmod(dest_path, 0o755)


# ── Tauri app ─────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def tauri_app():
    """Download, launch, and yield (proc, pid, base_url) for the Tauri test app."""
    _download_binary(_TAURI_DOWNLOAD_URL, _TAURI_BINARY_PATH)

    subprocess.run(["pkill", "-f", _TAURI_BINARY_NAME], check=False)
    time.sleep(0.5)

    proc = subprocess.Popen(
        [_TAURI_BINARY_PATH],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    pid = proc.pid
    time.sleep(3.0)

    base_url = "http://localhost:6769"
    yield proc, pid, base_url

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
    subprocess.run(["pkill", "-f", _TAURI_BINARY_NAME], check=False)


# ── Electron app ──────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def electron_app(html_server):
    """Download, launch, and yield (proc, pid, base_url) for the Electron test app."""
    app_path = os.path.join(_ELECTRON_APP_DIR, _ELECTRON_APP_NAME)
    _download_and_extract(_ELECTRON_DOWNLOAD_URL, _ELECTRON_APP_DIR, app_path)

    if not os.path.exists(app_path):
        pytest.skip("Electron app not found after download")

    test_page_url = f"{html_server}/test_page.html"

    subprocess.run(["pkill", "-f", "desktop-test-app-electron"], check=False)
    time.sleep(0.5)

    subprocess.run(
        ["launchctl", "setenv", "CUA_LOAD_URL", test_page_url],
        check=False,
    )

    proc = subprocess.Popen(
        ["open", "-a", app_path, "--args", "--force-renderer-accessibility"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(4.0)

    out = subprocess.run(
        ["pgrep", "-f", "desktop-test-app-electron"],
        capture_output=True, text=True,
    ).stdout.strip()
    pid = int(out.split("\n")[0]) if out else 0

    base_url = "http://localhost:6769"
    yield proc, pid, base_url

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
    subprocess.run(["pkill", "-f", "desktop-test-app-electron"], check=False)
