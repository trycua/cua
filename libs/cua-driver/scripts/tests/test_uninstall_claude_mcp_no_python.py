"""Regression coverage for safe MCP cleanup when Python is unavailable."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def _executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def test_claude_config_requires_python_before_release_is_mutated(tmp_path: Path) -> None:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    release_binary = home / ".cua-driver/packages/current/cua-driver"
    release_binary.parent.mkdir(parents=True, exist_ok=True)
    _executable(release_binary, "exit 0")

    launcher = home / ".local/bin/cua-driver"
    launcher.parent.mkdir(parents=True, exist_ok=True)
    launcher.symlink_to(release_binary)

    claude_json = home / ".claude.json"
    original = {
        "mcpServers": {
            "cua-computer-use": {
                "command": str(launcher),
                "args": ["mcp"],
            }
        }
    }
    claude_json.write_text(json.dumps(original), encoding="utf-8")

    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(
        fake_bin / "id",
        "if [ \"$1\" = -u ]; then printf '1000\\n'; else exit 1; fi",
    )

    env = os.environ.copy()
    env.update({"HOME": str(home), "PATH": str(fake_bin)})
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL)],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode != 0
    assert "python3 is required to safely inspect Claude MCP ownership" in result.stderr
    assert json.loads(claude_json.read_text(encoding="utf-8")) == original
    assert release_binary.exists()
    assert launcher.is_symlink()
