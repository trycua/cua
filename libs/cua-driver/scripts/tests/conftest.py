"""Isolation helpers for release-uninstaller script tests."""

from __future__ import annotations

import pytest


@pytest.fixture(autouse=True)
def isolate_release_process_inspection(request, monkeypatch):
    """Keep MCP tests independent of cua-driver processes on the host machine."""

    module = request.module
    if not module.__name__.endswith("test_uninstall_claude_mcp"):
        return
    run = getattr(module, "_run", None)
    executable = getattr(module, "_executable", None)
    if run is None or executable is None:
        return

    def isolated_run(tmp_path, *args, **kwargs):
        fake_bin = tmp_path / "fake-bin"
        executable(fake_bin / "pgrep", "exit 1")
        executable(
            fake_bin / "id",
            "if [ \"$1\" = -u ]; then printf '1000\\n'; else /usr/bin/id \"$@\"; fi",
        )
        return run(tmp_path, *args, **kwargs)

    monkeypatch.setattr(module, "_run", isolated_run)
