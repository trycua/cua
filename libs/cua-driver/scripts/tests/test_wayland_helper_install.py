from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

HELPER_DIR = Path(__file__).resolve().parents[2] / "wayland-helper"
INSTALL = HELPER_DIR / "install.sh"
BUNDLED_VERSION = json.loads((HELPER_DIR / "winrects@cua/metadata.json").read_text())["version"]


def _run(tmp_path: Path, *args: str) -> subprocess.CompletedProcess[str]:
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir(exist_ok=True)
    gsettings = fake_bin / "gsettings"
    gsettings.write_text("#!/bin/sh\n[ \"$1\" = get ] && echo \"@as []\"\nexit 0\n")
    gsettings.chmod(0o755)
    env = os.environ.copy()
    env.update({"HOME": str(tmp_path / "home"), "PATH": f"{fake_bin}:/usr/bin:/bin"})
    env.pop("XDG_DATA_HOME", None)
    return subprocess.run(
        ["/bin/bash", str(INSTALL), *args],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )


def _installed(tmp_path: Path) -> Path:
    return tmp_path / "home/.local/share/gnome-shell/extensions/winrects@cua"


def _preinstall(tmp_path: Path, version: int) -> None:
    dest = _installed(tmp_path)
    dest.mkdir(parents=True)
    (dest / "metadata.json").write_text(f'{{"uuid": "winrects@cua", "version": {version}}}\n')
    (dest / "extension.js").write_text(f"// helper v{version}\n")


def test_fresh_install_copies_the_bundled_helper(tmp_path: Path) -> None:
    result = _run(tmp_path)
    assert result.returncode == 0, result.stdout
    installed = json.loads((_installed(tmp_path) / "metadata.json").read_text())
    assert installed["version"] == BUNDLED_VERSION


@pytest.mark.parametrize("delta", [-1, 0], ids=["older", "same"])
def test_older_or_same_helper_is_replaced(tmp_path: Path, delta: int) -> None:
    _preinstall(tmp_path, BUNDLED_VERSION + delta)
    result = _run(tmp_path)
    assert result.returncode == 0, result.stdout
    assert (_installed(tmp_path) / "extension.js").read_text() == (
        HELPER_DIR / "winrects@cua/extension.js"
    ).read_text()


def test_newer_helper_from_another_app_is_kept(tmp_path: Path) -> None:
    newer = BUNDLED_VERSION + 1
    _preinstall(tmp_path, newer)
    result = _run(tmp_path)
    assert result.returncode == 0, result.stdout
    assert f"Keeping the installed winrects@cua v{newer}" in result.stdout
    assert (_installed(tmp_path) / "extension.js").read_text() == f"// helper v{newer}\n"

    forced = _run(tmp_path, "--force")
    assert forced.returncode == 0, forced.stdout
    installed = json.loads((_installed(tmp_path) / "metadata.json").read_text())
    assert installed["version"] == BUNDLED_VERSION
