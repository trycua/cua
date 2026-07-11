"""Ensure tool bins are on PATH for subprocess calls made during tests."""

from __future__ import annotations

import os


def pytest_configure(config):  # noqa: ARG001
    extra = [
        os.path.expanduser("~/.local/bin"),
        "/Applications/OrbStack.app/Contents/MacOS/xbin",  # OrbStack docker CLI
        "/opt/homebrew/bin",  # Homebrew (qemu, etc.)
    ]
    path_parts = os.environ.get("PATH", "").split(os.pathsep)
    for p in reversed(extra):
        if p not in path_parts:
            path_parts.insert(0, p)
    os.environ["PATH"] = os.pathsep.join(path_parts)
