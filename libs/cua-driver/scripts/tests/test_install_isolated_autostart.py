"""An isolated Windows install must not touch the machine's autostart task (#4090).

`install.ps1` with `CUA_DRIVER_RS_HOME` or `CUA_DRIVER_RS_INSTALL_DIR` set is an
isolated install. It leaves the shared `cua-driver-serve` task alone unless the
caller passes `-AutoStart` explicitly, and registration itself replaces an
existing task in one Task Scheduler call instead of deleting it first.
"""

from __future__ import annotations

import re
import shutil
import subprocess
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
INSTALL = REPO_ROOT / "libs/cua-driver/scripts/install.ps1"
AUTOSTART = REPO_ROOT / "libs/cua-driver/rust/crates/cua-driver/src/autostart.rs"

POWERSHELL = shutil.which("pwsh") or shutil.which("powershell")
requires_powershell = pytest.mark.skipif(POWERSHELL is None, reason="requires PowerShell")


def _register_script() -> str:
    source = AUTOSTART.read_text(encoding="utf-8")
    match = re.search(r'const REGISTER_PS: &str = r#"(.*?)"#;', source, re.S)
    assert match, "REGISTER_PS not found in autostart.rs"
    return match.group(1)


def _install_source() -> str:
    return INSTALL.read_text(encoding="utf-8-sig")


def test_registration_replaces_the_task_without_unregistering_it_first() -> None:
    script = _register_script()
    assert "Unregister-ScheduledTask" not in script
    assert "Register-ScheduledTask -Force" in script


def test_isolated_install_skips_every_autostart_branch_unless_requested() -> None:
    source = _install_source()
    assert (
        "$SkipIsolatedAutostart = $IsolatedInstall -and -not $AutoStartRequested" in source
    )
    # The skip must come first so neither registration nor the implicit
    # "existing task detected - re-registering" branch runs.
    chain = source.index("if ($SkipIsolatedAutostart) {")
    assert source.index("elseif ($AutoStart) {", chain) < source.index(
        "Existing 'cua-driver-serve' autostart task detected", chain
    )


def _autostart_request_probe() -> str:
    """The installer's own parameter block and request detection, verbatim."""
    source = _install_source()
    start = source.index("[CmdletBinding()]")
    end = source.index("$PSBoundParameters.ContainsKey('AutoStart')", start)
    end = source.index("\n", end)
    return source[start:end] + "\nWrite-Output ([bool]$AutoStartRequested)\n"


@requires_powershell
@pytest.mark.parametrize(
    ("arguments", "expected"),
    [
        ([], "False"),
        (["-AutoStart"], "True"),
        (["-AutoStart", "-NoAutoStart"], "False"),
        (["-NoAutoStart"], "False"),
    ],
)
def test_only_an_explicit_autostart_counts_as_a_request(
    tmp_path: Path, arguments: list[str], expected: str
) -> None:
    probe = tmp_path / "probe.ps1"
    probe.write_text(_autostart_request_probe(), encoding="utf-8")
    result = subprocess.run(
        [POWERSHELL, "-NoProfile", "-NonInteractive", "-File", str(probe), *arguments],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == expected, result.stdout + result.stderr
