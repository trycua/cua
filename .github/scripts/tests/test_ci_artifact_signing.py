"""Windows Authenticode signing no longer depends on a cached PowerShell module.

#5838: the `native win32-x64-msvc` job of `CD: cua SDK` twice failed at its
signing step with "The term 'Invoke-ArtifactSigning' is not recognized", 30 to
40 minutes into a release build, and passed on a re-run of the same commit.
Both failing logs show the same thing: azure/artifact-signing-action v2.0.0
caches its dependencies by default, the module cache key (ArtifactSigning-0.1.8)
names no architecture, the win32-arm64 job finished first and saved it, the
win32-x64 job restored that entry, the action skipped its install step because
of the hit, and PowerShell could not find the cmdlet. These tests keep the fix
honest: no signing step uses the cache, and the job proves the module works
before the build.
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github" / "workflows"
ACTION = ROOT / ".github" / "actions" / "artifact-signing-module" / "action.yml"
SIGNING_ACTION = "azure/artifact-signing-action@"
LOCAL_ACTION = "./.github/actions/artifact-signing-module"


def signing_steps():
    """Yield (workflow file, job id, step) for every use of the signing action."""
    for path in sorted(WORKFLOWS.glob("*.yml")):
        if SIGNING_ACTION not in path.read_text():
            continue
        data = yaml.safe_load(path.read_text())
        for job_id, job in data["jobs"].items():
            for step in job.get("steps", []):
                if str(step.get("uses", "")).startswith(SIGNING_ACTION):
                    yield path.name, job_id, step


def test_every_signing_step_is_found():
    found = {name for name, _, _ in signing_steps()}
    assert {"cd-cua-sdk.yml", "cd-cua-spaces.yml", "cd-cua-spacesd.yml", "cd-rust-cua-driver.yml"} <= found


def test_no_signing_step_restores_the_dependency_cache():
    for name, job, step in signing_steps():
        assert step["with"].get("cache-dependencies") == "false", (
            f"{name} {job} {step.get('name')!r} must set cache-dependencies: 'false' "
            "(a cached module from the other architecture's job skipped the install, #5838)"
        )


def test_every_signing_step_pins_the_same_commit():
    pins = {step["uses"] for _, _, step in signing_steps()}
    assert len(pins) == 1, pins
    assert re.fullmatch(SIGNING_ACTION + r"[0-9a-f]{40}", next(iter(pins)))


def test_the_sdk_windows_job_checks_the_module_before_the_build():
    data = yaml.safe_load((WORKFLOWS / "cd-cua-sdk.yml").read_text())
    steps = data["jobs"]["native"]["steps"]
    names = [s.get("name", s.get("uses", "")) for s in steps]
    prep = next(i for i, s in enumerate(steps) if s.get("uses") == LOCAL_ACTION)
    build = names.index("Build libcua_sdk, the cua CLI and the N-API runtime")
    sign = next(i for i, s in enumerate(steps) if str(s.get("uses", "")).startswith(SIGNING_ACTION))
    checkout = next(i for i, s in enumerate(steps) if str(s.get("uses", "")).startswith("actions/checkout"))
    assert checkout < prep < build < sign
    # The same runs that sign are the ones that check, no others.
    assert steps[prep]["if"] == steps[sign]["if"] == "runner.os == 'Windows' && vars.AZURE_CLIENT_ID != ''"


def test_the_module_version_is_the_one_the_pinned_action_installs():
    action = yaml.safe_load(ACTION.read_text())
    assert action["inputs"]["module-version"]["default"] == "0.1.8"
    assert action["runs"]["steps"][0]["shell"] == "pwsh"


# The script of the composite action, run for real with PowerShell when this
# machine has it (GitHub's Linux and Windows runners do), against stand-ins for
# the PowerShell Gallery and for the fresh `pwsh` process the action starts (a
# function named pwsh wins over the executable, so the check is answered here).
PWSH = shutil.which("pwsh")

PRELUDE = r"""
function Install-Module { param($Name, $RequiredVersion, [switch]$Force, $Repository)
  Write-Host "INSTALL_CALL $Name $RequiredVersion"
  $script:calls = 1 + [int]$script:calls
  if ($script:calls -le [int]$env:FAKE_INSTALL_FAILURES) { throw 'PSGallery: connection reset' } }
function Import-Module { param($Name, $RequiredVersion, [switch]$Force) }
function Get-Command { param($Name, $ErrorAction)
  if ($env:FAKE_NO_CMDLET -eq '1') { return $null }
  [pscustomobject]@{ Source = 'ArtifactSigning'; Version = '0.1.8' } }
function Start-Sleep { param($Seconds) Write-Host "SLEEP $Seconds" }
function pwsh { $global:LASTEXITCODE = [int]$env:FAKE_FRESH_EXIT }
"""


def run_script(**fake):
    script = yaml.safe_load(ACTION.read_text())["runs"]["steps"][0]["run"]
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        ps1 = tmp / "step.ps1"
        ps1.write_text(PRELUDE + script)
        env = dict(os.environ, MODULE_VERSION="0.1.8", ATTEMPTS="3", FAKE_FRESH_EXIT="0")
        env.update({k: str(v) for k, v in fake.items()})
        done = subprocess.run(
            [PWSH, "-NoProfile", "-NonInteractive", "-File", str(ps1)],
            capture_output=True, text=True, env=env, timeout=120,
        )
    return done.returncode, done.stdout + done.stderr


needs_pwsh = pytest.mark.skipif(PWSH is None or sys.platform == "win32", reason="needs pwsh on a Unix host")


@needs_pwsh
def test_a_healthy_module_passes():
    code, out = run_script()
    assert code == 0, out
    assert out.count("INSTALL_CALL") == 1
    assert "A new PowerShell process resolves Invoke-ArtifactSigning." in out


@needs_pwsh
def test_a_gallery_error_is_retried_and_then_passes():
    code, out = run_script(FAKE_INSTALL_FAILURES=2)
    assert code == 0, out
    assert out.count("INSTALL_CALL") == 3
    assert "SLEEP 5" in out and "SLEEP 10" in out


@needs_pwsh
def test_a_gallery_that_stays_down_fails_with_a_clear_message():
    code, out = run_script(FAKE_INSTALL_FAILURES=99)
    assert code == 1, out
    assert out.count("INSTALL_CALL") == 3
    assert "::error title=Artifact Signing module::ArtifactSigning 0.1.8 is not usable after 3 attempts (PSGallery: connection reset)" in out


@needs_pwsh
def test_a_module_without_the_cmdlet_fails_the_job():
    code, out = run_script(FAKE_NO_CMDLET=1)
    assert code == 1, out
    assert "has no Invoke-ArtifactSigning" in out


@needs_pwsh
def test_a_new_process_that_cannot_find_the_cmdlet_fails_the_job():
    code, out = run_script(FAKE_FRESH_EXIT=1)
    assert code == 1, out
    assert "a new PowerShell process does not find Invoke-ArtifactSigning" in out
