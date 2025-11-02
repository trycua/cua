import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Optional, Dict, Any
from urllib import request
from urllib.error import HTTPError, URLError
import psutil

# Map child PID -> listening port
_pid_to_port: Dict[int, int] = {}


def _post_json(url: str, payload: Dict[str, Any]) -> Dict[str, Any]:
    data = json.dumps(payload).encode("utf-8")
    req = request.Request(url, data=data, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with request.urlopen(req, timeout=5) as resp:
            text = resp.read().decode("utf-8")
            return json.loads(text)
    except HTTPError as e:
        try:
            body = (e.read() or b"").decode("utf-8", errors="ignore")
            return json.loads(body)
        except Exception:
            return {"error": "http_error", "status": getattr(e, 'code', None)}
    except URLError as e:
        return {"error": "url_error", "reason": str(e.reason)}


def _detect_port_for_pid(pid: int) -> int:
    """Detect a listening local TCP port for the given PID using psutil.

    Fails fast if psutil is unavailable or if no suitable port is found.
    """
    if psutil is None:
        raise RuntimeError("psutil is required for PID->port detection. Please install psutil.")

    # Scan system-wide connections and filter by PID
    for c in psutil.net_connections(kind="tcp"):
        if getattr(c, "pid", None) != pid:
            continue
        laddr = getattr(c, "laddr", None)
        status = str(getattr(c, "status", ""))
        if not laddr or not isinstance(laddr, tuple) or len(laddr) < 2:
            continue
        lip, lport = laddr[0], int(laddr[1])
        if status.upper() != "LISTEN":
            continue
        if lip in ("127.0.0.1", "::1", "0.0.0.0", "::"):
            return lport

    raise RuntimeError(f"Could not detect listening port for pid {pid}")


def launch_window(
    url: Optional[str] = None,
    *,
    html: Optional[str] = None,
    title: str = "Window",
    x: Optional[int] = None,
    y: Optional[int] = None,
    width: int = 600,
    height: int = 400,
    icon: Optional[str] = None,
    use_inner_size: bool = False,
    title_bar_style: str = "default",
) -> int:
    """Create a pywebview window in a child process and return its PID.

    Preferred input is a URL via the positional `url` parameter.
    To load inline HTML instead, pass `html=...`.

    Spawns `python -m bench_ui.child` with a JSON config passed via a temp file.
    The child prints a single JSON line: {"pid": <pid>, "port": <port>}.
    We cache pid->port for subsequent control calls like get_element_rect.
    """
    if not url and not html:
        raise ValueError("launch_window requires either a url or html")

    config = {
        "url": url,
        "html": html,
        "title": title,
        "x": x,
        "y": y,
        "width": width,
        "height": height,
        "icon": icon,
        "use_inner_size": use_inner_size,
        "title_bar_style": title_bar_style,
    }

    with tempfile.NamedTemporaryFile("w", delete=False, suffix=".json") as f:
        json.dump(config, f)
        cfg_path = f.name

    try:
        # Launch child process
        proc = subprocess.Popen(
            [sys.executable, "-m", "bench_ui.child", cfg_path],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        assert proc.stdout is not None
        # Read first line with startup info
        line = proc.stdout.readline().strip()
        info = json.loads(line)
        pid = int(info["pid"]) if "pid" in info else proc.pid
        port = int(info["port"])  # required
        _pid_to_port[pid] = port
        return pid
    finally:
        try:
            os.unlink(cfg_path)
        except Exception:
            pass


def get_element_rect(pid: int, selector: str, *, space: str = "window"):
    """Ask the child process to compute element client rect via injected JS.

    Returns a dict like {"x": float, "y": float, "width": float, "height": float} or None if not found.
    """
    if pid not in _pid_to_port:
        _pid_to_port[pid] = _detect_port_for_pid(pid)
    port = _pid_to_port[pid]
    url = f"http://127.0.0.1:{port}/rect"
    last: Dict[str, Any] = {}
    for _ in range(30):  # ~3s total
        resp = _post_json(url, {"selector": selector, "space": space})
        last = resp or {}
        rect = last.get("rect") if isinstance(last, dict) else None
        err = last.get("error") if isinstance(last, dict) else None
        if rect is not None:
            return rect
        if err in ("window_not_ready", "invalid_json"):
            time.sleep(0.1)
            continue
        # If other transient errors, brief retry
        if err:
            time.sleep(0.1)
            continue
        time.sleep(0.1)
    raise RuntimeError(f"Failed to get element rect: {last}")


def execute_javascript(pid: int, javascript: str):
    """Execute arbitrary JavaScript in the window and return its result.

    Retries briefly while the window is still becoming ready.
    """
    if pid not in _pid_to_port:
        _pid_to_port[pid] = _detect_port_for_pid(pid)
    port = _pid_to_port[pid]
    url = f"http://127.0.0.1:{port}/eval"
    last: Dict[str, Any] = {}
    for _ in range(30):  # ~3s total
        resp = _post_json(url, {"javascript": javascript})
        last = resp or {}
        if isinstance(last, dict):
            if "result" in last:
                return last["result"]
            if last.get("error") in ("window_not_ready", "invalid_json"):
                time.sleep(0.1)
                continue
            if last.get("error"):
                time.sleep(0.1)
                continue
        time.sleep(0.1)
    raise RuntimeError(f"Failed to execute JavaScript: {last}")
