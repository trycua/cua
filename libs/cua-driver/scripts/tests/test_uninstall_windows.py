from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys

import pytest


pytestmark = pytest.mark.skipif(
    sys.platform != "win32", reason="native Windows PowerShell required"
)
UNINSTALL = Path(__file__).resolve().parents[1] / "uninstall.ps1"
FIXTURE = Path(__file__).with_name("uninstall-windows-fixture.ps1")
DEFAULT_LOCAL_CLI = "localappdata/Programs/Cua/cua-driver-local/bin/cua-driver-local.exe"


def _run_uninstall(
    root: Path,
    overrides: dict[str, str],
    *,
    invocation: str = "File",
    uninstaller: Path = UNINSTALL,
) -> subprocess.CompletedProcess[str]:
    for directory in ("profile", "localappdata", "appdata", "bin", "temp", "modules"):
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
        "PSModulePath": str(root / "modules"),
        "CUA_DRIVER_RS_UNINSTALL_FORCE": "1",
        **overrides,
    }
    try:
        return subprocess.run(
            [
                str(powershell),
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-File",
                str(FIXTURE),
                "-UninstallerPath",
                str(uninstaller),
                "-FixtureRoot",
                str(root),
                "-Invocation",
                invocation,
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


@pytest.mark.parametrize(
    ("local_path", "overrides", "invocation"),
    [
        pytest.param(DEFAULT_LOCAL_CLI, {}, "Expression", id="default-cli"),
        pytest.param(
            "custom bin/cua-driver-local.exe",
            {"CUA_DRIVER_LOCAL_INSTALL_DIR": "custom bin"},
            "Expression",
            id="configured-cli",
        ),
        pytest.param(
            "profile/.cua-driver-local/packages/current/cua-driver-local.exe",
            {},
            "Expression",
            id="default-marker",
        ),
        pytest.param(
            "custom home/packages/current/cua-driver-local.exe",
            {"CUA_DRIVER_LOCAL_HOME": "custom home"},
            "Expression",
            id="configured-marker",
        ),
        pytest.param("bin/cua-driver-local.exe", {}, "Expression", id="path-cli"),
        pytest.param(None, {}, "Expression", id="marker-free-home"),
        pytest.param(DEFAULT_LOCAL_CLI, {}, "File", id="file-entrypoint"),
    ],
)
def test_release_uninstall_reports_and_preserves_local_product(
    tmp_path: Path, local_path: str | None, overrides: dict[str, str], invocation: str
) -> None:
    root = tmp_path / "fixture with spaces"
    release_cli = root / "profile/.cua-driver/packages/current/cua-driver.exe"
    local_files = [
        root / overrides.get("CUA_DRIVER_LOCAL_HOME", "profile/.cua-driver-local") / "config.json"
    ]
    if local_path is not None:
        local_files.append(root / local_path)
    for path in (release_cli, *local_files):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture payload\n", encoding="utf-8")

    result = _run_uninstall(
        root, {key: str(root / value) for key, value in overrides.items()}, invocation=invocation
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert not release_cli.exists()
    for path in local_files:
        assert path.read_text(encoding="utf-8") == "fixture payload\n"
    notice = "source-built cua-driver-local installation remains"
    if local_path is None:
        assert notice not in result.stdout
    else:
        assert notice in result.stdout
        assert str(root / local_path) in result.stdout
        assert ".\\libs\\cua-driver\\scripts\\uninstall-local.ps1" in result.stdout


@pytest.mark.parametrize(
    "command",
    [
        r"Remove-Item -LiteralPath '..\outside.txt' -Force",
        "Stop-Process -Id $PID -Force",
        r"Start-Process -FilePath '.\missing.exe'",
        "Get-Process -Name fixture-not-running",
        "schtasks.exe /Delete /TN fixture-nonexistent /F",
    ],
)
def test_fixture_refuses_host_operations_even_when_caught(tmp_path: Path, command: str) -> None:
    outside = tmp_path / "outside.txt"
    outside.write_text("keep\n", encoding="utf-8")
    probe = tmp_path / "probe.ps1"
    probe.write_text(f"try {{ {command} }} catch {{}}\n", encoding="utf-8")

    result = _run_uninstall(tmp_path / "fixture", {}, uninstaller=probe)

    assert result.returncode != 0
    assert "fixture refused host operation" in result.stderr
    assert outside.read_text(encoding="utf-8") == "keep\n"
