"""Off-Space layouts for the OS-* tasks (Amendment 18): the task's app on one desktop Space, the user on another.

A task's task.json may set `off_space`:

* `"desktop"`: the task's apps open on desktop Space 1; the harness then moves to desktop Space 2 (Control-Right, as a
  user would) and opens the user's window there. The agent's app is on a Space that is not on screen.
* `"fullscreen"`: the task's app window is put into full screen (its own Space); the harness then moves back to
  desktop Space 1 and opens the user's window there.

Everything happens before the sentinel is armed, through `benchspaces` (tools/benchspaces.swift, built by
swift/build.sh). Spaces are never created here: the VM is provisioned with two desktop Spaces (A18.6).
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

LAYOUTS = ("desktop", "fullscreen")


class SpaceError(RuntimeError):
    pass


def tool(build_dir: Path) -> Path:
    return Path(build_dir) / "BenchSpaces.app"


def run(build_dir: Path, *args: str, timeout: float = 40) -> dict[str, Any]:
    """Start the helper through LaunchServices, so it runs with its own Accessibility grant, and read its JSON."""
    import tempfile

    with tempfile.TemporaryDirectory(prefix="benchspaces-") as tmp:
        out = Path(tmp) / "out.json"
        done = subprocess.run(
            ["/usr/bin/open", "-W", "-g", "-n", "-a", str(tool(build_dir)), "--args", "--out", str(out), *args],
            capture_output=True, text=True, timeout=timeout,
        )
        try:
            data = json.loads(out.read_text()) if out.exists() else {}
        except ValueError:
            data = {}
    data.setdefault("rc", 1 if not data else data.get("rc", 0))
    if done.returncode != 0:
        data["open_rc"] = done.returncode
        data["rc"] = data.get("rc") or done.returncode
    if done.stderr.strip():
        data["stderr"] = done.stderr.strip()[-300:]
    return data


def desktops(listing: dict[str, Any]) -> list[dict[str, Any]]:
    return [s for s in listing.get("spaces", []) if s.get("type") == 0]


def before_apps(build_dir: Path, layout: str) -> dict[str, Any]:
    """Make desktop Space 1 active before the task's apps open."""
    if layout not in LAYOUTS:
        raise SpaceError(f"unknown off_space layout {layout!r}")
    listing = run(build_dir, "list")
    if layout == "desktop" and len(desktops(listing)) < 2:
        raise SpaceError(f"off_space desktop needs two desktop Spaces, found {len(desktops(listing))}")
    moved = run(build_dir, "goto", "1")
    if moved.get("rc") != 0:
        raise SpaceError(f"could not reach Space 1: {moved}")
    return {"layout": layout, "start": moved}


def after_apps(build_dir: Path, layout: str, target_pid: int | None, target_title: str) -> dict[str, Any]:
    """Move the user away from the task's app: to desktop 2, or out of the app's full-screen Space to desktop 1."""
    out: dict[str, Any] = {"layout": layout}
    if layout == "fullscreen":
        if target_pid is None:
            raise SpaceError("full-screen layout needs the target app's pid")
        fs = run(build_dir, "fullscreen", str(target_pid), target_title)
        out["fullscreen"] = fs
        if fs.get("rc") != 0:
            raise SpaceError(f"full screen failed: {fs}")
        user = run(build_dir, "goto", "1")
    else:
        user = run(build_dir, "goto", "2")
    out["user_space"] = user
    if user.get("rc") != 0:
        raise SpaceError(f"could not move to the user's Space: {user}")
    return out


def reset(build_dir: Path) -> dict[str, Any]:
    """After a trial: back to desktop Space 1 (full-screen Spaces close with their app)."""
    return run(build_dir, "goto", "1")
