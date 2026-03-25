"""Ensure ~/.local/bin is on PATH so tools installed there (cua, lume, uv) are found."""

from __future__ import annotations

import os


def pytest_configure(config):  # noqa: ARG001
    local_bin = os.path.expanduser("~/.local/bin")
    path = os.environ.get("PATH", "")
    if local_bin not in path.split(os.pathsep):
        os.environ["PATH"] = local_bin + os.pathsep + path
