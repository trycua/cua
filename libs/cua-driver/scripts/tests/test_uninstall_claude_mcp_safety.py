"""Regression coverage for release-uninstaller Claude MCP safety gaps."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def _executable(path: Path, body: str = "exit 0") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def _release_fixture(tmp_path: Path, *, pgrep_body: str = "exit 1") -> tuple[Path, Path, Path, dict[str, str]]:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    release_binary = home / ".cua-driver/packages/current/cua-driver"
    _executable(release_binary)

    launcher = home / ".local/bin/cua-driver"
    launcher.parent.mkdir(parents=True, exist_ok=True)
    launcher.symlink_to(release_binary)

    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pgrep", pgrep_body)
    _executable(
        fake_bin / "id",
        "if [ \"$1\" = -u ]; then printf '1000\\n'; else /usr/bin/id \"$@\"; fi",
    )
    _executable(fake_bin / "pkill")
    _executable(fake_bin / "systemctl")

    env = os.environ.copy()
    env.update({"HOME": str(home), "PATH": f"{fake_bin}:/usr/bin:/bin"})
    return home, release_binary, launcher, env


def _run(env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["/bin/bash", str(UNINSTALL)],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


def test_legacy_substring_in_unrelated_argument_is_preserved(tmp_path: Path) -> None:
    home, _, _, env = _release_fixture(tmp_path)
    claude_json = home / ".claude.json"
    original = {
        "mcpServers": {
            "notes-server": {
                "command": "/usr/bin/other",
                "args": ["/work/cua-driver-rs-notes"],
            }
        }
    }
    claude_json.write_text(json.dumps(original), encoding="utf-8")

    result = _run(env)

    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(claude_json.read_text(encoding="utf-8")) == original


def test_malformed_claude_config_fails_before_release_or_daemon_mutation(tmp_path: Path) -> None:
    pgrep_marker = tmp_path / "pgrep-called"
    home, release_binary, launcher, env = _release_fixture(
        tmp_path,
        pgrep_body=f"printf called > {pgrep_marker}; exit 1",
    )
    claude_json = home / ".claude.json"
    malformed = '{"mcpServers":'
    claude_json.write_text(malformed, encoding="utf-8")

    result = _run(env)

    assert result.returncode != 0
    assert "could not read Claude config" in result.stderr
    assert claude_json.read_text(encoding="utf-8") == malformed
    assert release_binary.exists()
    assert launcher.is_symlink()
    assert not pgrep_marker.exists(), "daemon inspection ran before Claude config validation"
    assert "cua-driver uninstalled." not in result.stdout
