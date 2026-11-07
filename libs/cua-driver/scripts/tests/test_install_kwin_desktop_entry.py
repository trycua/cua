"""KWin grants window capture only to an executable named by a desktop entry.

The release installer writes that entry, naming the stable `packages/current`
path, and the uninstaller removes only the entry it owns. KWin is a fake here;
the grant itself is proved on real KWin. That the release
installer calls this is proved in test_cua_driver_installer_integrity.py, which
runs the whole installer.
"""

from __future__ import annotations

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


def write_entry(
    tmp_path: Path,
    executable: str,
    *,
    cli: str = "/home/test/.local/bin/cua-driver",
    kwin: bool = True,
    data_home: Path | None = None,
    failing_write: bool = False,
):
    # PATH holds only these links, so a real KWin on the test host stays invisible.
    home, fake_bin = tmp_path / "home", tmp_path / "bin"
    home.mkdir(exist_ok=True)
    fake_bin.mkdir(exist_ok=True)
    for tool in ("mkdir", "mv", "rm", "sed"):
        link = fake_bin / tool
        if not link.exists():
            link.symlink_to(shutil.which(tool))
    if kwin:
        fake(fake_bin / "kwin_wayland")
    env = {"HOME": str(home), "PATH": str(fake_bin)}
    if data_home is not None:
        env["XDG_DATA_HOME"] = str(data_home)
    # `printf` is a builtin, so a failed write is simulated by shadowing it.
    fail = 'printf() { return 1; }\n' if failing_write else ""
    script = f". {shlex.quote(str(COMMON))}\n{fail}cua_driver_write_kwin_desktop_entry {shlex.quote(executable)} {shlex.quote(cli)}\n"
    return home, subprocess.run(
        ["/bin/bash", "-euc", script], env=env, cwd=tmp_path, text=True, capture_output=True, check=False
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
        "X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,org_kde_kwin_keystate",
        "X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2",
        "X-Cua-Cli=/home/test/.local/bin/cua-driver",
    ]
    assert [p.name for p in (home / ENTRY).parent.iterdir()] == [Path(ENTRY).name]


def test_the_install_log_says_what_the_entry_grants(tmp_path: Path) -> None:
    home, result = write_entry(tmp_path, "/opt/cua-driver")
    assert result.stdout.splitlines() == [
        f"==> wrote KWin desktop entry {home / ENTRY}",
        "==> KWin lets this driver capture windows with no prompt. uninstall.sh removes the entry.",
    ]


def test_entry_follows_xdg_data_home(tmp_path: Path) -> None:
    data_home = tmp_path / "data"
    home, result = write_entry(tmp_path, "/opt/cua-driver", data_home=data_home)
    assert result.returncode == 0, result.stderr
    assert (data_home / "applications/ai.cua.driver.release.desktop").exists()
    assert not (home / ENTRY).exists()


def test_no_entry_without_kwin(tmp_path: Path) -> None:
    home, result = write_entry(tmp_path, "/opt/cua-driver", kwin=False)
    assert result.returncode == 0, result.stderr
    assert not (home / ENTRY).exists()


def test_rerun_replaces_a_stale_entry(tmp_path: Path) -> None:
    home, _ = write_entry(tmp_path, "/old/cua-driver")
    _, result = write_entry(tmp_path, "/new/cua-driver")
    assert result.returncode == 0, result.stderr
    assert 'Exec="/new/cua-driver" serve' in (home / ENTRY).read_text().splitlines()
    assert "/old/" not in (home / ENTRY).read_text()


def live_install(tmp_path: Path, name: str) -> tuple[Path, Path]:
    executable = tmp_path / name / "cua-driver"
    executable.parent.mkdir()
    fake(executable)
    cli = tmp_path / f"{name}-bin/cua-driver"
    cli.parent.mkdir()
    cli.symlink_to(executable)
    return executable, cli


def test_a_second_install_with_its_own_cli_keeps_the_entry_of_a_live_install(tmp_path: Path) -> None:
    live, live_cli = live_install(tmp_path, "live")
    home, _ = write_entry(tmp_path, str(live), cli=str(live_cli))
    _, result = write_entry(tmp_path, "/tmp/throwaway/cua-driver", cli="/tmp/throwaway-bin/cua-driver")
    assert result.returncode == 0, result.stderr
    assert f'Exec="{live}" serve' in (home / ENTRY).read_text().splitlines()
    assert result.stdout == f"==> kept KWin desktop entry: it grants {live}\n"


def test_a_relative_cli_path_is_recorded_absolute(tmp_path: Path) -> None:
    home, result = write_entry(tmp_path, "/opt/cua-driver", cli="bin/cua-driver")
    assert result.returncode == 0, result.stderr
    assert f"X-Cua-Cli={tmp_path}/bin/cua-driver" in (home / ENTRY).read_text().splitlines()


def test_a_relative_cli_link_still_counts_as_live(tmp_path: Path) -> None:
    live, live_cli = live_install(tmp_path, "live")
    home, _ = write_entry(tmp_path, str(live), cli=str(live_cli))
    live_cli.unlink()
    live_cli.symlink_to("../live/cua-driver")
    _, result = write_entry(tmp_path, "/tmp/throwaway/cua-driver", cli="/tmp/throwaway-bin/cua-driver")
    assert result.returncode == 0, result.stderr
    assert f'Exec="{live}" serve' in (home / ENTRY).read_text().splitlines()


def test_a_second_install_that_takes_the_cli_takes_the_entry(tmp_path: Path) -> None:
    first, cli = live_install(tmp_path, "first")
    home, _ = write_entry(tmp_path, str(first), cli=str(cli))
    second = tmp_path / "second/cua-driver"
    second.parent.mkdir()
    fake(second)
    cli.unlink()
    cli.symlink_to(second)
    _, result = write_entry(tmp_path, str(second), cli=str(cli))
    assert result.returncode == 0, result.stderr
    assert f'Exec="{second}" serve' in (home / ENTRY).read_text().splitlines()


def test_rerunning_the_install_after_kwin_appears_writes_the_entry(tmp_path: Path) -> None:
    home, _ = write_entry(tmp_path, "/opt/cua-driver", kwin=False)
    assert not (home / ENTRY).exists()
    _, result = write_entry(tmp_path, "/opt/cua-driver")
    assert result.returncode == 0, result.stderr
    assert 'Exec="/opt/cua-driver" serve' in (home / ENTRY).read_text().splitlines()


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


def test_a_failed_write_leaves_no_temp_file(tmp_path: Path) -> None:
    home, result = write_entry(tmp_path, "/opt/cua-driver", failing_write=True)
    assert result.returncode == 1
    applications = home / ENTRY
    assert [p.name for p in applications.parent.iterdir()] == []


def owned(home: Path, executable: Path) -> Path:
    entry = home / ENTRY
    entry.parent.mkdir(parents=True, exist_ok=True)
    entry.write_text(f'[Desktop Entry]\nExec="{executable}" serve\n')
    return entry


def test_uninstall_removes_its_own_entry(release_install) -> None:
    entry = owned(release_install.home, release_install.release)
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
    entry = data_home / "applications/ai.cua.driver.release.desktop"
    entry.parent.mkdir(parents=True)
    entry.write_text(f'[Desktop Entry]\nExec="{release_install.release}" serve\n')
    release_install.env["XDG_DATA_HOME"] = str(data_home)
    result = release_install.run()
    assert result.returncode == 0, result.stderr
    assert not entry.exists()
