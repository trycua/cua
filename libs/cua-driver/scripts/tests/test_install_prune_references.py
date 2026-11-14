"""Retention GC must name hand-written files that pin a release dir (#3957).

`prune_old_releases` keeps the N most recent per-version release dirs. A user
who wrote a systemd unit or desktop entry against a versioned path is not using
the stable `current` symlink, so the next GC silently breaks that file: the
unit restarts into `Failed at step EXEC` with no install-time signal. The
installer cannot rewrite files it does not own, but it can name the referencing
file and the stable replacement before the dir is deleted.

The function is exercised in isolation: sourcing `_install-rust.sh` would run a
full install, so the two shell functions are extracted verbatim and driven
against a disposable `$HOME`.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[4]
INSTALLER = REPO_ROOT / "libs/cua-driver/scripts/_install-rust.sh"
TARGET = "x86_64-unknown-linux-gnu"

pytestmark = pytest.mark.skipif(not INSTALLER.is_file(), reason="release installer is not present")


def _extract_function(source: str, name: str) -> str:
    lines = source.splitlines()
    start = next(
        (index for index, line in enumerate(lines) if line.startswith(f"{name}()")),
        None,
    )
    assert start is not None, f"{name} is missing from {INSTALLER.name}"
    end = next(
        (index for index in range(start + 1, len(lines)) if lines[index] == "}"),
        None,
    )
    assert end is not None, f"{name} has no closing brace"
    return "\n".join(lines[start : end + 1])


@pytest.fixture
def runner(tmp_path: Path) -> Path:
    source = INSTALLER.read_text(encoding="utf-8")
    script = tmp_path / "run-prune.sh"
    script.write_text(
        "\n".join(
            [
                "#!/usr/bin/env bash",
                "set -euo pipefail",
                'log() { printf "==> %s\\n" "$*"; }',
                'BINARY_NAME="cua-driver"',
                _extract_function(source, "warn_referenced_release_dirs"),
                _extract_function(source, "prune_old_releases"),
                'prune_old_releases "$@"',
                "",
            ]
        ),
        encoding="utf-8",
    )
    script.chmod(0o755)
    return script


def _home(tmp_path: Path) -> Path:
    home = tmp_path / "home"
    (home / ".cua-driver/packages/releases").mkdir(parents=True)
    (home / ".config/systemd/user").mkdir(parents=True)
    (home / ".local/share/applications").mkdir(parents=True)
    return home


def _release(home: Path, version: str, mtime: int) -> Path:
    path = home / ".cua-driver/packages/releases" / f"{version}-{TARGET}"
    path.mkdir()
    binary = path / "cua-driver"
    binary.write_text("#!/bin/sh\n", encoding="utf-8")
    binary.chmod(0o755)
    os.utime(path, (mtime, mtime))
    return path


def _current(home: Path, version: str) -> Path:
    link = home / ".cua-driver/packages/current"
    link.symlink_to(f"releases/{version}-{TARGET}")
    return link


def _prune(runner: Path, home: Path, keep: int) -> subprocess.CompletedProcess[str]:
    packages = home / ".cua-driver/packages"
    return subprocess.run(
        [str(runner), str(packages / "releases"), str(packages / "current"), TARGET, str(keep)],
        capture_output=True,
        text=True,
        check=True,
        env={
            **os.environ,
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / ".config"),
        },
    )


def _three_releases(home: Path) -> tuple[Path, Path]:
    """Three per-target releases; keep=2 prunes the oldest, keeps the current."""

    oldest = _release(home, "0.9.0", 1_000)
    _release(home, "1.0.0", 2_000)
    _release(home, "1.1.0", 3_000)
    _current(home, "1.1.0")
    return oldest, home / ".cua-driver/packages/current"


@pytest.mark.parametrize(
    "relative_path",
    [
        ".config/systemd/user/cua-driver.service",
        ".config/systemd/user/cua-driver.socket",
        ".config/systemd/user/cua-driver.timer",
        ".config/autostart/cua-driver.desktop",
        ".local/share/applications/cua-driver.desktop",
    ],
)
def test_pruning_names_each_referencing_file_and_the_stable_path(
    runner: Path, tmp_path: Path, relative_path: str
) -> None:
    home = _home(tmp_path)
    oldest, current = _three_releases(home)
    reference = home / relative_path
    reference.parent.mkdir(parents=True, exist_ok=True)
    # The unit spells the versioned dir through `%h`; desktop entries tend to
    # spell it absolutely. Matching is on the `packages/releases/<dir>`
    # suffix, so either form has to hit.
    spelling = (
        "%h/.cua-driver" if relative_path.endswith((".service", ".socket", ".timer")) else home
    )
    reference.write_text(
        f"ExecStart={spelling}/packages/releases/{oldest.name}/cua-driver serve\n",
        encoding="utf-8",
    )

    completed = _prune(runner, home, keep=2)

    assert not oldest.exists(), "retention behaviour must not change"
    assert f"warning: pruning {oldest}" in completed.stderr
    assert str(reference) in completed.stderr
    assert f"warning: point those references at {current}/cua-driver" in completed.stderr


def test_no_warning_when_nothing_references_the_pruned_release(
    runner: Path, tmp_path: Path
) -> None:
    home = _home(tmp_path)
    oldest, _ = _three_releases(home)

    completed = _prune(runner, home, keep=2)

    assert not oldest.exists()
    assert "warning:" not in completed.stderr


def test_reference_to_a_retained_release_is_not_reported(runner: Path, tmp_path: Path) -> None:
    home = _home(tmp_path)
    oldest, _ = _three_releases(home)
    retained = home / ".cua-driver/packages/releases" / f"1.0.0-{TARGET}"
    desktop = home / ".local/share/applications/cua-driver.desktop"
    desktop.write_text(
        f"Exec={retained}/cua-driver serve\n",
        encoding="utf-8",
    )

    completed = _prune(runner, home, keep=2)

    assert not oldest.exists()
    assert retained.exists()
    assert "warning:" not in completed.stderr


def test_active_release_is_never_pruned_even_with_a_reference(runner: Path, tmp_path: Path) -> None:
    home = _home(tmp_path)
    # The current symlink points at the oldest dir (a rollback); keep=1 would
    # otherwise prune it, and the warning must not claim it is going away.
    active = _release(home, "0.9.0", 1_000)
    _release(home, "1.0.0", 2_000)
    current = _current(home, "0.9.0")
    desktop = home / ".local/share/applications/cua-driver.desktop"
    desktop.write_text(f"Exec={active}/cua-driver serve\n", encoding="utf-8")

    completed = _prune(runner, home, keep=1)

    assert active.exists()
    assert current.exists()
    assert "warning:" not in completed.stderr
