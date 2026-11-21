"""The installer adds BIN_DIR to the shell rc file once, however often it runs.

Runs the real PATH block of `_install-rust.sh` several times with BIN_DIR off
PATH, the way a systemd timer update does (#6335).
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest

INSTALLER = Path(__file__).resolve().parents[1] / "_install-rust.sh"


def _path_block() -> str:
    source = INSTALLER.read_text(encoding="utf-8")
    start = source.index("# Auto-extend PATH for users")
    end = source.index('echo "cua-driver-rs $VERSION installed."')
    return source[start:end]


@pytest.mark.skipif(
    os.name != "posix" or shutil.which("bash") is None,
    reason="requires a posix host with bash",
)
def test_repeated_runs_append_the_path_line_once(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()
    bin_dir = home / ".local" / "bin"
    script = tmp_path / "harness.sh"
    script.write_text(
        "set -euo pipefail\n"
        f'BIN_DIR="{bin_dir}"\nNO_MODIFY_PATH=0\n'
        "log() { printf '==> %s\\n' \"$*\"; }\n"
        "print_path_hint() { :; }\n" + _path_block(),
        encoding="utf-8",
    )
    env = {"HOME": str(home), "SHELL": "/bin/bash", "PATH": os.environ["PATH"]}
    for _ in range(3):
        result = subprocess.run(["bash", str(script)], capture_output=True, text=True, env=env, check=False)
        assert result.returncode == 0, result.stdout + result.stderr

    lines = (home / ".bashrc").read_text(encoding="utf-8").splitlines()
    assert lines.count(f'export PATH="{bin_dir}:$PATH"') == 1
