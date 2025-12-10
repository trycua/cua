import time

import psutil
import pytest
from bench_ui import execute_javascript, launch_window
from bench_ui.api import _pid_to_port

HTML = """
<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>Bench UI Test</title>
  </head>
  <body>
    <div id="t">hello-world</div>
  </body>
</html>
"""


def test_execute_js_after_clearing_port_mapping():
    # Skip if pywebview backend is unavailable on this machine
    pywebview = pytest.importorskip("webview")

    pid = launch_window(html=HTML, title="Bench UI Test", width=400, height=300)
    try:
        # Give a brief moment for window to render and server to start
        time.sleep(1.0)

        # Sanity: mapping should exist initially
        assert pid in _pid_to_port

        # Clear the cached mapping to simulate a fresh process lookup
        del _pid_to_port[pid]

        # Now execute JS; this should succeed by detecting the port via psutil
        result = execute_javascript(pid, "document.querySelector('#t')?.textContent")
        assert result == "hello-world"
    finally:
        # Best-effort cleanup of the child process
        try:
            p = psutil.Process(pid)
            p.terminate()
            try:
                p.wait(timeout=3)
            except psutil.TimeoutExpired:
                p.kill()
        except Exception:
            pass
