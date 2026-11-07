"""KWin grants window capture only to an executable named by a desktop entry.

The release installer writes that entry, naming the stable `packages/current`
path, and the uninstaller removes only the entry it owns. KWin and kbuildsycoca6
are fakes here; the grant itself is proved on real KWin.
"""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[4]
COMMON = ROOT / "libs/cua-driver/scripts/_install-common.sh"
ENTRY = ".local/share/applications/ai.cua.driver.release.desktop"


def fake(path: Path, body: str = "exit 0") -> None:
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def write_entry(tmp_path: Path, executable: str, *, kwin: bool = True, sycoca: str = "exit 0"):
    # PATH holds only these links, so a real KWin on the test host stays invisible.
    home, fake_bin = tmp_path / "home", tmp_path / "bin"
    home.mkdir(exist_ok=True)
    fake_bin.mkdir(exist_ok=True)
    for tool in ("mkdir", "mv", "rm", "touch"):
        link = fake_bin / tool
        if not link.exists():
            link.symlink_to(shutil.which(tool))
    if kwin:
        fake(fake_bin / "kwin_wayland")
    fake(fake_bin / "kbuildsycoca6", f'touch "$HOME/sycoca-ran"\n{sycoca}')
    env = {"HOME": str(home), "PATH": str(fake_bin)}
    script = f". {shlex.quote(str(COMMON))}\ncua_driver_write_kwin_desktop_entry {shlex.quote(executable)}\n"
    return home, subprocess.run(
        ["/bin/bash", "-euc", script], env=env, text=True, capture_output=True, check=False
    )


def test_entry_names_the_stable_path_and_both_grants(tmp_path: Path) -> None:
    executable = "/home/test/.cua-driver/packages/current/cua-driver"
    home, result = write_entry(tmp_path, executable)
    assert result.returncode == 0, result.stderr
    assert (home / ENTRY).read_text().splitlines() == [
        "[Desktop Entry]",
        "Type=Application",
        "Name=Cua Driver",
        f'Exec="{executable}" serve',
        "NoDisplay=true",
        "X-KDE-Wayland-Interfaces=org_kde_plasma_window_management",
        "X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2",
    ]
    assert (home / "sycoca-ran").exists()
    assert [p.name for p in (home / ENTRY).parent.iterdir()] == [Path(ENTRY).name]


def test_no_entry_without_kwin(tmp_path: Path) -> None:
    home, result = write_entry(tmp_path, "/opt/cua-driver", kwin=False)
    assert result.returncode == 0, result.stderr
    assert not (home / ENTRY).exists()
    assert not (home / "sycoca-ran").exists()


def test_rerun_replaces_a_stale_entry(tmp_path: Path) -> None:
    home, _ = write_entry(tmp_path, "/old/cua-driver")
    _, result = write_entry(tmp_path, "/new/cua-driver")
    assert result.returncode == 0, result.stderr
    assert 'Exec="/new/cua-driver" serve' in (home / ENTRY).read_text().splitlines()
    assert "/old/" not in (home / ENTRY).read_text()


def test_path_with_a_space_stays_one_argument(tmp_path: Path) -> None:
    home, result = write_entry(tmp_path, "/home/a b/cua-driver")
    assert result.returncode == 0, result.stderr
    assert 'Exec="/home/a b/cua-driver" serve' in (home / ENTRY).read_text().splitlines()


@pytest.mark.parametrize(
    "executable",
    ["relative/cua-driver", '/a"b/cua-driver', "/a`b/cua-driver", "/a$b/cua-driver",
     "/a\\b/cua-driver", "/a%b/cua-driver", "/a\nb/cua-driver"],
)
def test_path_an_exec_line_cannot_carry_writes_nothing(tmp_path: Path, executable: str) -> None:
    home, result = write_entry(tmp_path, executable)
    assert result.returncode == 0, result.stderr
    assert "skipped KWin desktop entry" in result.stdout
    assert not (home / ENTRY).exists()


def test_sycoca_failure_does_not_fail_the_install(tmp_path: Path) -> None:
    home, result = write_entry(tmp_path, "/opt/cua-driver", sycoca="exit 7")
    assert result.returncode == 0, result.stderr
    assert (home / ENTRY).exists()


def test_release_installer_writes_the_entry_after_the_swap() -> None:
    text = (ROOT / "libs/cua-driver/scripts/_install-rust.sh").read_text()
    swap = text.index('log "current -> releases/${VERSION}-${TARGET}"')
    call = text.index('cua_driver_write_kwin_desktop_entry "$CURRENT_LINK/$BINARY_NAME" || true')
    assert swap < call
    assert "cua_driver_write_kwin_desktop_entry() { :; }" in text


def owned(home: Path, executable: Path) -> Path:
    entry = home / ENTRY
    entry.parent.mkdir(parents=True, exist_ok=True)
    entry.write_text(f'[Desktop Entry]\nExec="{executable}" serve\n')
    return entry


def test_uninstall_removes_its_own_entry(release_install) -> None:
    entry = owned(release_install.home, release_install.release)
    fake(release_install.bin / "kbuildsycoca6", 'touch "$HOME/sycoca-ran"')
    result = release_install.run()
    assert result.returncode == 0, result.stderr
    assert not entry.exists()
    assert (release_install.home / "sycoca-ran").exists()


def test_uninstall_keeps_an_entry_it_does_not_own(release_install) -> None:
    entry = owned(release_install.home, Path("/opt/other/cua-driver"))
    result = release_install.run()
    assert result.returncode == 0, result.stderr
    assert entry.exists()
