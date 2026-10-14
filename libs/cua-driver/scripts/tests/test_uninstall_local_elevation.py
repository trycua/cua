"""Elevation contract for the Windows local uninstaller.

`install-local.ps1` registers the `cua-driver-local-serve` autostart task
through `cua-driver-local autostart enable`, which self-elevates via
ShellExecute 'runas' and registers the task at RunLevel=Highest. A
non-elevated process can therefore neither delete the task nor stop the High-IL
daemon it spawned, so `uninstall-local.ps1` re-execs itself elevated instead of
dead-ending with "rerun from an elevated PowerShell".

The decision logic is factored into small PowerShell functions so it can be
exercised here without ever firing a UAC prompt. The re-exec itself
(`Start-Process -Verb RunAs`) is interactive by nature: it is covered by source
assertions on shape and ordering, not by an executed test.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
SCRIPTS = REPO_ROOT / "libs/cua-driver/scripts"
UNINSTALL_LOCAL = SCRIPTS / "uninstall-local.ps1"

POWERSHELL = shutil.which("pwsh") or shutil.which("powershell")
requires_powershell = pytest.mark.skipif(POWERSHELL is None, reason="requires PowerShell")
requires_windows_powershell = pytest.mark.skipif(
    os.name != "nt" or POWERSHELL is None,
    reason="requires Windows with PowerShell",
)


def _source() -> str:
    return UNINSTALL_LOCAL.read_text(encoding="utf-8-sig")


def _extract_function(source: str, name: str) -> str:
    """Return the full text of a PowerShell function, braces balanced."""
    start = source.index(f"function {name}")
    depth = 0
    for index in range(source.index("{", start), len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"unterminated PowerShell function {name}")


def _run_powershell(tmp_path: Path, body: str) -> subprocess.CompletedProcess[str]:
    script = tmp_path / "elevation-harness.ps1"
    script.write_text(
        "Set-StrictMode -Version Latest\n$ErrorActionPreference = 'Stop'\n" + body,
        encoding="utf-8",
    )
    return subprocess.run(
        [str(POWERSHELL), "-NoProfile", "-NonInteractive", "-File", str(script)],
        capture_output=True,
        text=True,
        check=True,
    )


def _sandbox_env(tmp_path: Path) -> dict[str, str]:
    """Redirect every path the uninstaller resolves into tmp_path."""
    home = tmp_path / "profile"
    local_app_data = tmp_path / "localappdata"
    roaming = tmp_path / "roaming"
    for path in (home, local_app_data, roaming):
        path.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update(
        {
            "USERPROFILE": str(home),
            "LOCALAPPDATA": str(local_app_data),
            "APPDATA": str(roaming),
            "CUA_DRIVER_LOCAL_HOME": str(home / ".cua-driver-local"),
            "CUA_DRIVER_LOCAL_INSTALL_DIR": str(
                local_app_data / "Programs/Cua/cua-driver-local/bin"
            ),
        }
    )
    return env


# ---------- Source shape and ordering -------------------------------------


def test_local_uninstall_self_elevates_like_the_released_uninstaller() -> None:
    source = _source()
    for token in (
        "function Test-IsElevated",
        "function Test-NeedsElevation",
        "-Verb RunAs",
        "-PassThru",
        "-Wait",
        "exit $elevated.ExitCode",
        "$MyInvocation.MyCommand.Path",
    ):
        assert token in source, token


def test_elevation_is_decided_before_any_teardown() -> None:
    # The pre-existing contract is that the script fails before touching
    # anything rather than falling through into a partial removal. Elevation
    # has to be decided in front of the first destructive call, not after it.
    source = _source()
    decision = source.index("if (-not (Test-IsElevated) -and (Test-NeedsElevation))")
    for destructive in ("schtasks.exe /End", "schtasks.exe /Delete", "Remove-Item", "Stop-Process"):
        assert decision < source.index(destructive), destructive


def test_validate_only_returns_before_the_elevation_pre_check() -> None:
    # --validate-only is a pure reporting mode; it must never raise UAC.
    source = _source()
    assert source.index("if ($ValidateOnly)") < source.index(
        "if (-not (Test-IsElevated) -and (Test-NeedsElevation))"
    )


def test_declined_elevation_keeps_the_failsafe() -> None:
    source = _source()
    # A cancelled UAC prompt surfaces as an InvalidOperationException out of
    # Start-Process; it must be caught and reported, never a raw stack trace.
    catch_block = source[
        source.index("-Verb RunAs") : source.index("$taskExists = Test-LocalTaskExists")
    ]
    assert "} catch {" in catch_block
    assert "nothing was removed" in catch_block
    assert "rerun this script from an elevated PowerShell" in catch_block
    assert "exit 1" in catch_block
    # And the original refusal is still the fallback once teardown starts.
    assert 'throw "could not remove $TaskName; rerun from an elevated PowerShell"' in source


def test_elevated_child_is_started_with_force() -> None:
    source = _source()
    assert "Get-ElevationArgumentList -ScriptPath $scriptPath -ForceMode $true" in source


# ---------- Executed decision logic ---------------------------------------


@pytest.mark.parametrize(
    ("has_task", "has_daemon", "expected"),
    [
        (False, False, "no"),
        (True, False, "yes"),
        (False, True, "yes"),
        (True, True, "yes"),
    ],
)
@requires_powershell
def test_needs_elevation_truth_table(
    tmp_path: Path,
    has_task: bool,
    has_daemon: bool,
    expected: str,
) -> None:
    body = "\n".join(
        [
            f"function Test-LocalTaskExists {{ return ${str(has_task).lower()} }}",
            f"function Test-LocalDaemonRunning {{ return ${str(has_daemon).lower()} }}",
            _extract_function(_source(), "Test-NeedsElevation"),
            "if (Test-NeedsElevation) { Write-Output 'yes' } else { Write-Output 'no' }",
        ]
    )
    result = _run_powershell(tmp_path, body)
    assert result.stdout.strip() == expected


@pytest.mark.parametrize(("force_mode", "expects_force"), [(True, True), (False, False)])
@requires_powershell
def test_force_mode_crosses_the_elevation_boundary(
    tmp_path: Path,
    force_mode: bool,
    expects_force: bool,
) -> None:
    # Losing -Force would strand the elevated child on the Read-Host
    # confirmation in a window the user is not looking at, while the parent
    # blocks in Start-Process -Wait.
    script_path = r"C:\Users\someone\path with space\uninstall-local.ps1"
    body = "\n".join(
        [
            _extract_function(_source(), "Get-ElevationArgumentList"),
            f"$list = Get-ElevationArgumentList -ScriptPath '{script_path}' "
            f"-ForceMode ${str(force_mode).lower()}",
            'foreach ($item in $list) { Write-Output "arg=$item" }',
        ]
    )
    result = _run_powershell(tmp_path, body)
    arguments = [
        line[len("arg=") :] for line in result.stdout.splitlines() if line.startswith("arg=")
    ]

    assert arguments[:4] == ["-ExecutionPolicy", "Bypass", "-NoProfile", "-File"]
    # The script path stays a single argument even with spaces in it.
    assert arguments[4] == script_path
    assert ("-Force" in arguments) is expects_force


ELEVATION_CHECK = (
    "    ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]"
    "::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"
)
TASK_QUERY = (
    "        & schtasks.exe /Query /TN $TaskName 2>$null | Out-Null\n"
    "        return ($LASTEXITCODE -eq 0)"
)
PARAM_BLOCK = "param([switch]$Force, [switch]$ValidateOnly)"


def _rehearsal_copy(tmp_path: Path, start_process_body: str) -> Path:
    """A copy of the uninstaller that rehearses the re-exec without UAC.

    Only three things are replaced: "am I elevated" (no), "does the task
    exist" (yes), and Start-Process itself. Everything between them - the
    ordering, the argument list, the exit-code handling - is the real script.
    """
    source = _source()
    for original in (ELEVATION_CHECK, TASK_QUERY, PARAM_BLOCK):
        assert source.count(original) == 1, original
    patched = (
        source.replace(ELEVATION_CHECK, "    return $false")
        .replace(TASK_QUERY, "        return $true")
        .replace(
            PARAM_BLOCK,
            PARAM_BLOCK + "\nfunction Start-Process {\n" + start_process_body + "\n}\n",
        )
    )
    # A space in the path proves the re-exec keeps it a single argument.
    script = tmp_path / "rehearsed uninstall-local.ps1"
    script.write_text(patched, encoding="utf-8")
    return script


@requires_powershell
def test_reexec_forwards_force_and_propagates_the_child_exit_code(tmp_path: Path) -> None:
    probe = tmp_path / "start-process.log"
    script = _rehearsal_copy(
        tmp_path,
        "    [CmdletBinding()]\n"
        "    param([string]$FilePath, [string[]]$ArgumentList, [string]$Verb,\n"
        "          [switch]$PassThru, [switch]$Wait)\n"
        f"    Set-Content -LiteralPath '{probe}' "
        "-Value (@($FilePath, $Verb) + $ArgumentList)\n"
        "    return [pscustomobject]@{ ExitCode = 7 }",
    )
    result = subprocess.run(
        [str(POWERSHELL), "-NoProfile", "-NonInteractive", "-File", str(script), "-Force"],
        env=_sandbox_env(tmp_path),
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 7, result.stdout + result.stderr
    recorded = probe.read_text(encoding="utf-8").split()
    assert recorded[0].endswith("powershell.exe")
    assert recorded[1] == "RunAs"
    # -Force must survive the boundary: without it the elevated child blocks
    # on the Read-Host confirmation in a window nobody is watching.
    assert recorded[-1] == "-Force"
    assert str(script) in probe.read_text(encoding="utf-8")


@requires_powershell
def test_declined_uac_changes_nothing_and_reports_an_actionable_message(tmp_path: Path) -> None:
    script = _rehearsal_copy(
        tmp_path,
        "    [CmdletBinding()]\n"
        "    param([string]$FilePath, [string[]]$ArgumentList, [string]$Verb,\n"
        "          [switch]$PassThru, [switch]$Wait)\n"
        "    throw [System.InvalidOperationException]::new("
        "'The operation was canceled by the user.')",
    )
    home = Path(_sandbox_env(tmp_path)["CUA_DRIVER_LOCAL_HOME"])
    marker = home / "packages/current/cua-driver-local.exe"
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text("local\n", encoding="utf-8")

    result = subprocess.run(
        [str(POWERSHELL), "-NoProfile", "-NonInteractive", "-File", str(script), "-Force"],
        env=_sandbox_env(tmp_path),
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 1, result.stdout + result.stderr
    assert "The operation was canceled by the user." in result.stdout
    assert "nothing was removed" in result.stdout
    assert "rerun this script from an elevated PowerShell" in result.stdout
    # The install is untouched: a cancelled prompt is not a partial removal.
    assert marker.exists()
    assert "removed scheduled task" not in result.stdout


@requires_powershell
def test_uninstall_local_parses(tmp_path: Path) -> None:
    body = "\n".join(
        [
            "$errors = $null",
            "$tokens = $null",
            "[System.Management.Automation.Language.Parser]::ParseFile("
            f"'{UNINSTALL_LOCAL}', [ref]$tokens, [ref]$errors) | Out-Null",
            "if ($errors) { $errors | ForEach-Object { Write-Output $_.ToString() } }",
            "else { Write-Output 'parse-ok' }",
        ]
    )
    result = _run_powershell(tmp_path, body)
    assert result.stdout.strip() == "parse-ok"


@requires_powershell
def test_script_path_is_populated_when_invoked_from_a_file(tmp_path: Path) -> None:
    # The re-exec depends on this: unlike uninstall.ps1 under `irm | iex`,
    # uninstall-local.ps1 is always started from a file path, so it can hand
    # its own path to Start-Process -Verb RunAs without materializing a copy.
    probe = tmp_path / "probe.ps1"
    probe.write_text("Write-Output $MyInvocation.MyCommand.Path\n", encoding="utf-8")
    result = subprocess.run(
        [str(POWERSHELL), "-NoProfile", "-NonInteractive", "-File", str(probe)],
        capture_output=True,
        text=True,
        check=True,
    )
    assert Path(result.stdout.strip()) == probe


# ---------- Windows behavior ----------------------------------------------


@requires_windows_powershell
def test_validate_only_does_not_elevate(tmp_path: Path) -> None:
    result = subprocess.run(
        [
            str(POWERSHELL),
            "-NoProfile",
            "-NonInteractive",
            "-File",
            str(UNINSTALL_LOCAL),
            "-ValidateOnly",
        ],
        env=_sandbox_env(tmp_path),
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "task=cua-driver-local-serve" in result.stdout
    assert "UAC" not in result.stdout


def _skip_when_a_real_local_install_is_present() -> None:
    """Never run the destructive path against a developer's real install.

    The autostart task and the daemon process list are machine-global: no env
    sandbox can hide them. If either is present the uninstaller would correctly
    ask for elevation, which must not happen from a test run.
    """
    query = subprocess.run(
        ["schtasks.exe", "/Query", "/TN", "cua-driver-local-serve"],
        capture_output=True,
        text=True,
        check=False,
    )
    if query.returncode == 0:
        pytest.skip("a real cua-driver-local-serve task is registered on this machine")
    listing = subprocess.run(
        ["tasklist.exe", "/FI", "IMAGENAME eq cua-driver-local.exe"],
        capture_output=True,
        text=True,
        check=False,
    )
    if "cua-driver-local.exe" in listing.stdout:
        pytest.skip("a real cua-driver-local daemon is running on this machine")


@requires_windows_powershell
def test_uninstall_runs_unelevated_when_nothing_needs_admin(tmp_path: Path) -> None:
    # No autostart task and no running daemon, so the pre-check must not ask
    # for elevation — a false positive here would put a UAC prompt in front of
    # every local uninstall.
    _skip_when_a_real_local_install_is_present()
    result = subprocess.run(
        [str(POWERSHELL), "-NoProfile", "-NonInteractive", "-File", str(UNINSTALL_LOCAL), "-Force"],
        env=_sandbox_env(tmp_path),
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "triggering UAC prompt" not in result.stdout
    assert "cua-driver-local uninstalled" in result.stdout
