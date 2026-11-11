"""KWin grants window capture only to an executable named by a desktop entry.

`cua-driver kwin-capture on` writes that entry; the installer writes none. The
uninstaller removes it only when it names this install. That the release
installer prints the hint on a KWin host is proved in
test_cua_driver_installer_integrity.py, which runs the whole installer.
"""

from __future__ import annotations

from pathlib import Path

import pytest

ENTRY = ".local/share/applications/ai.cua.driver.desktop"


def owned(home: Path, executable: Path) -> Path:
    entry = home / ENTRY
    entry.parent.mkdir(parents=True, exist_ok=True)
    entry.write_text(f'[Desktop Entry]\nExec="{executable}" serve\n')
    return entry


@pytest.mark.parametrize("named", ["current", "release"])
def test_uninstall_removes_the_entry_naming_this_install(release_install, named: str) -> None:
    packages = release_install.home / ".cua-driver/packages"
    executable = {
        "current": packages / "current/cua-driver",
        "release": packages / "releases/0.41.3-x86_64-unknown-linux-gnu/cua-driver",
    }[named]
    entry = owned(release_install.home, executable)
    result = release_install.run()
    assert result.returncode == 0, result.stderr
    assert not entry.exists()


def test_uninstall_keeps_an_entry_it_does_not_own(release_install) -> None:
    entry = owned(release_install.home, Path("/opt/other/cua-driver"))
    result = release_install.run()
    assert result.returncode == 0, result.stderr
    assert entry.exists()


def test_uninstall_keeps_an_entry_that_only_mentions_its_exec_line(release_install) -> None:
    entry = owned(release_install.home, release_install.release)
    exec_line = f'Exec="{release_install.release}" serve'
    entry.write_text(f"[Desktop Entry]\nExec=/opt/other {exec_line}\n")
    result = release_install.run()
    assert result.returncode == 0, result.stderr
    assert entry.exists()


def test_uninstall_follows_xdg_data_home(release_install, tmp_path: Path) -> None:
    data_home = tmp_path / "data"
    entry = data_home / "applications/ai.cua.driver.desktop"
    entry.parent.mkdir(parents=True)
    entry.write_text(f'[Desktop Entry]\nExec="{release_install.release}" serve\n')
    release_install.env["XDG_DATA_HOME"] = str(data_home)
    result = release_install.run()
    assert result.returncode == 0, result.stderr
    assert not entry.exists()
