from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys

import pytest


pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="native Windows PowerShell required")
UNINSTALL = Path(__file__).resolve().parents[1] / "uninstall.ps1"
FIXTURE = Path(__file__).with_name("uninstall-windows-fixture.ps1")


def _run_uninstall(root: Path, overrides: dict[str, str]) -> subprocess.CompletedProcess[str]:
    for directory in ("profile", "localappdata", "appdata", "bin", "temp"):
        (root / directory).mkdir(parents=True, exist_ok=True)
    system_root = Path(os.environ["SystemRoot"])
    powershell = system_root / "System32/WindowsPowerShell/v1.0/powershell.exe"
    env = {
        "SystemRoot": str(system_root),
        "USERPROFILE": str(root / "profile"),
        "LOCALAPPDATA": str(root / "localappdata"),
        "APPDATA": str(root / "appdata"),
        "TEMP": str(root / "temp"),
        "TMP": str(root / "temp"),
        "PATH": str(root / "bin"),
        "PATHEXT": ".EXE",
        "CUA_DRIVER_RS_UNINSTALL_FORCE": "1",
        **overrides,
    }
    try:
        return subprocess.run(
            [
                str(powershell), "-NoLogo", "-NoProfile", "-NonInteractive", "-File",
                str(FIXTURE), "-UninstallerPath", str(UNINSTALL), "-FixtureRoot", str(root),
            ],
            cwd=root,
            env=env,
            stdin=subprocess.DEVNULL,
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        pytest.fail(f"uninstaller timed out: stdout={error.stdout!r}; stderr={error.stderr!r}")


def test_release_uninstall_reports_and_preserves_local_cli(tmp_path: Path) -> None:
    root = tmp_path / "fixture with spaces"
    release_cli = root / "profile/.cua-driver/packages/current/cua-driver.exe"
    local_cli = root / "localappdata/Programs/Cua/cua-driver-local/bin/cua-driver-local.exe"
    local_config = root / "profile/.cua-driver-local/config.json"
    for path in (release_cli, local_cli, local_config):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture payload\n", encoding="utf-8")

    result = _run_uninstall(root, {})

    assert result.returncode == 0, result.stdout + result.stderr
    assert not release_cli.exists()
    assert local_cli.read_text(encoding="utf-8") == "fixture payload\n"
    assert local_config.read_text(encoding="utf-8") == "fixture payload\n"
    assert "source-built cua-driver-local installation remains" in result.stdout
    assert str(local_cli) in result.stdout
    assert ".\\libs\\cua-driver\\scripts\\uninstall-local.ps1" in result.stdout
