"""The installer's shell-rc PATH append must be idempotent (issue #5694).

_install-rust.sh runs under `curl ... | bash`, which never sources .zshrc, so
the installer's own $PATH cannot observe the PATH a user's shell config
already sets. Deciding whether to append solely on the process $PATH meant
every rerun wrote another `# Added by cua-driver-rs installer` block.

The contract: reruns append nothing once the rc file already covers BIN_DIR,
whether the installer wrote the block itself or the user exported the dir by
hand, and --no-modify-path still disables the append entirely.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

requires_posix_bash = pytest.mark.skipif(
    os.name != "posix" or not Path("/bin/bash").exists(),
    reason="requires a posix host with /bin/bash",
)

REPO_ROOT = Path(__file__).resolve().parents[4]
UNIX_INSTALLER = REPO_ROOT / "libs/cua-driver/scripts/_install-rust.sh"
MARKER = "Added by cua-driver-rs installer"


def _extract_path_block() -> str:
    lines = UNIX_INSTALLER.read_text(encoding="utf-8").splitlines()
    start = next(
        (
            i
            for i, line in enumerate(lines)
            if line == 'if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then'
        ),
        None,
    )
    assert start is not None, "could not locate the PATH-append block in _install-rust.sh"
    end = next((i for i in range(start, len(lines)) if lines[i] == "fi"), None)
    assert end is not None, "could not locate the end of the PATH-append block"
    return "\n".join(lines[start : end + 1]) + "\n"


def _run_path_block(
    tmp_path: Path, home: Path, overrides: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    block = tmp_path / "path-block.sh"
    block.write_text(_extract_path_block(), encoding="utf-8")
    env = {
        "HOME": str(home),
        "SHELL": "/bin/zsh",
        # BIN_DIR deliberately absent from PATH, as under `curl | bash`,
        # cron, or a non-login ssh that never sourced the user's rc file.
        "PATH": "/usr/bin:/bin",
        "BIN_DIR": str(home / ".local" / "bin"),
        "NO_MODIFY_PATH": "0",
    }
    env.update(overrides or {})
    return subprocess.run(
        [
            "/bin/bash",
            "-c",
            'set -euo pipefail; log() { :; }; print_path_hint() { :; }; . "$1"',
            "_",
            str(block),
        ],
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


@requires_posix_bash
def test_repeated_installer_runs_append_the_path_block_once(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()

    first = _run_path_block(tmp_path, home)
    second = _run_path_block(tmp_path, home)
    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr

    zshrc = (home / ".zshrc").read_text(encoding="utf-8")
    assert zshrc.count(MARKER) == 1
    bin_dir = str(home / ".local" / "bin")
    assert zshrc.count(f'export PATH="{bin_dir}:$PATH"') == 1


@requires_posix_bash
@pytest.mark.parametrize(
    "export_line",
    [
        'export PATH="{bin_dir}:$PATH"',
        'export PATH="$HOME/.local/bin:$PATH"',
        'export PATH="~/.local/bin:$PATH"',
    ],
    ids=["absolute", "home-relative", "tilde"],
)
def test_existing_export_in_rc_file_prevents_append(
    tmp_path: Path, export_line: str
) -> None:
    home = tmp_path / "home"
    home.mkdir()
    bin_dir = str(home / ".local" / "bin")
    (home / ".zshrc").write_text(
        export_line.format(bin_dir=bin_dir) + "\n", encoding="utf-8"
    )

    result = _run_path_block(tmp_path, home)
    assert result.returncode == 0, result.stderr

    zshrc = (home / ".zshrc").read_text(encoding="utf-8")
    assert MARKER not in zshrc


@requires_posix_bash
def test_commented_out_export_does_not_prevent_append(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()
    bin_dir = str(home / ".local" / "bin")
    (home / ".zshrc").write_text(
        f'# export PATH="{bin_dir}:$PATH"\n', encoding="utf-8"
    )

    result = _run_path_block(tmp_path, home)
    assert result.returncode == 0, result.stderr

    zshrc = (home / ".zshrc").read_text(encoding="utf-8")
    assert zshrc.count(MARKER) == 1


@requires_posix_bash
def test_bash_shell_targets_bashrc(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()

    result = _run_path_block(tmp_path, home, overrides={"SHELL": "/bin/bash"})
    assert result.returncode == 0, result.stderr
    assert MARKER in (home / ".bashrc").read_text(encoding="utf-8")
    assert not (home / ".zshrc").exists()


@requires_posix_bash
def test_no_modify_path_still_disables_the_append(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()

    result = _run_path_block(tmp_path, home, overrides={"NO_MODIFY_PATH": "1"})
    assert result.returncode == 0, result.stderr
    assert not (home / ".zshrc").exists()


@requires_posix_bash
def test_rerun_with_a_different_bin_dir_still_appends(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()

    first = _run_path_block(tmp_path, home)
    second = _run_path_block(
        tmp_path, home, overrides={"BIN_DIR": str(home / "other" / "bin")}
    )
    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr

    zshrc = (home / ".zshrc").read_text(encoding="utf-8")
    assert zshrc.count(MARKER) == 2
    assert f'export PATH="{home}/other/bin:$PATH"' in zshrc
