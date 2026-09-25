"""The canonical-installer canary runs what users run and files one issue on failure."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import textwrap

import pytest
import yaml


REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github/workflows/monitor-branded-installers.yml"


@pytest.fixture(scope="module")
def workflow() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def step(job: dict, name: str) -> dict:
    matches = [entry for entry in job["steps"] if entry.get("name") == name]
    assert len(matches) == 1, name
    return matches[0]


def test_canary_runs_often_and_after_every_installer_change(workflow: dict) -> None:
    triggers = workflow[True]  # PyYAML parses `on` as True
    assert triggers["schedule"] == [{"cron": "15 */6 * * *"}]
    assert triggers["release"] == {"types": ["published"]}
    assert triggers["workflow_run"] == {
        "workflows": ["CD: Cua Driver (cross-platform)"],
        "types": ["completed"],
    }
    paths = triggers["push"]["paths"]
    assert triggers["push"]["branches"] == ["main"]
    for required in (
        ".github/release-state/cua-driver-rs-*",
        "libs/cua-driver/scripts/_install-rust.sh",
        "libs/cua-driver/scripts/install.sh",
        "libs/cua-driver/scripts/install.ps1",
    ):
        assert required in paths
    assert "pull_request" not in triggers and "pull_request_target" not in triggers
    assert workflow["permissions"] == {"contents": "read", "issues": "write"}


def test_canary_uses_default_resolution_on_every_platform(workflow: dict) -> None:
    canary = workflow["jobs"]["canary"]
    assert canary["strategy"]["matrix"]["os"] == ["ubuntu-latest", "macos-26", "windows-latest"]
    assert canary["env"]["INSTALLER_SOURCE"] == "${{ github.event_name == 'push' && 'checkout' || 'cua.ai' }}"
    assert canary["env"]["CUA_DRIVER_RS_TELEMETRY_ENABLED"] == "false"
    # The only pin is the manual diagnostics input; scheduled runs never pin.
    assert canary["env"]["CANARY_PIN"] == "${{ inputs.version }}"

    unix = step(canary, "Run the canonical macOS/Linux installer with default resolution")["run"]
    assert '/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"' in unix
    assert "unset CUA_DRIVER_RS_VERSION CUA_DRIVER_VERSION GH_TOKEN GITHUB_TOKEN" in unix
    assert unix.index("unset CUA_DRIVER_RS_VERSION") < unix.index('export CUA_DRIVER_RS_VERSION="$CANARY_PIN"')
    for isolation in ('export HOME="$CANARY/home"', 'export CUA_DRIVER_RS_HOME=', 'export CUA_DRIVER_RS_INSTALL_DIR="$CANARY/bin"'):
        assert isolation in unix
    assert 'grep -Fqx "source=Notarized Developer ID"' in unix
    assert "\\(YCK386LBJ7\\)$" in unix
    assert "codesign --verify --deep --strict" in unix

    windows = step(canary, "Run the canonical Windows installer with default resolution")["run"]
    assert "[scriptblock]::Create((Invoke-RestMethod https://cua.ai/driver/install.ps1))) -NoAutoStart -NoPathUpdate" in windows
    assert "& ./libs/cua-driver/scripts/install.ps1 -NoAutoStart -NoPathUpdate" in windows
    assert "Get-AuthenticodeSignature" in windows
    assert "$env:CUA_DRIVER_RS_HOME = Join-Path $canary" in windows


def test_failures_open_one_bug_issue_per_canary(workflow: dict) -> None:
    report = workflow["jobs"]["report"]
    assert set(report["needs"]) == {"check", "canary"}
    assert report["if"] == "always() && (needs.check.result == 'failure' || needs.canary.result == 'failure')"
    run = step(report, "Open or update monitoring issues")["run"]
    assert "--label bug" in run
    assert "Cua Driver canonical installer canary is failing" in run
    assert "Branded installer endpoint check is failing" in run
    assert "actions/runs/${GITHUB_RUN_ID}" in run


def _run_report(
    tmp_path: Path, workflow: dict, *, open_issues: str, env: dict[str, str]
) -> tuple[list[list[str]], str]:
    run = step(workflow["jobs"]["report"], "Open or update monitoring issues")["run"]
    calls = tmp_path / "gh-calls"
    script = tmp_path / "report.sh"
    script.write_text(
        textwrap.dedent(
            f"""\
            gh() {{
              printf '%s\\037' "$@" >> "{calls.as_posix()}"
              printf '\\036' >> "{calls.as_posix()}"
              if [[ "$1 $2" == "issue list" ]]; then
                printf '%s' "$OPEN_ISSUE"
              fi
            }}
            """
        )
        + run,
        encoding="utf-8",
    )
    result = subprocess.run(
        ["bash", str(script)],
        capture_output=True,
        text=True,
        env={
            "PATH": os.environ["PATH"],
            "GITHUB_REPOSITORY": "trycua/cua",
            "GITHUB_RUN_ID": "42",
            "GITHUB_SHA": "abc123",
            "TRIGGER": "schedule",
            "DRY_RUN": "false",
            "ENDPOINT_RESULT": "success",
            "ENDPOINT_FAILURES": "",
            "CANARY_RESULT": "success",
            "OPEN_ISSUE": open_issues,
            **env,
        },
    )
    assert result.returncode == 0, result.stderr
    records = calls.read_text().split("\x1e") if calls.exists() else []
    return [record.strip("\x1f").split("\x1f") for record in records if record], result.stdout


def test_report_creates_then_comments_instead_of_duplicating(tmp_path: Path, workflow: dict) -> None:
    calls, _ = _run_report(tmp_path, workflow, open_issues="", env={"CANARY_RESULT": "failure"})
    assert calls[0][:5] == ["issue", "list", "--repo", "trycua/cua", "--state"]
    assert calls[1][:8] == [
        "issue", "create", "--repo", "trycua/cua",
        "--title", "Cua Driver canonical installer canary is failing",
        "--label", "bug",
    ]
    assert calls[1][8] == "--body"
    assert "Workflow run: https://github.com/trycua/cua/actions/runs/42" in calls[1][9]
    assert len(calls) == 2

    (tmp_path / "gh-calls").unlink()
    calls, _ = _run_report(tmp_path, workflow, open_issues="4200", env={"CANARY_RESULT": "failure"})
    assert [call[:3] for call in calls if call[1] != "list"] == [["issue", "comment", "4200"]]


def test_report_dry_run_files_nothing(tmp_path: Path, workflow: dict) -> None:
    calls, stdout = _run_report(
        tmp_path,
        workflow,
        open_issues="",
        env={"CANARY_RESULT": "failure", "ENDPOINT_RESULT": "failure", "ENDPOINT_FAILURES": "x returned HTTP 500", "DRY_RUN": "true"},
    )
    assert calls == []
    assert 'would open or update the issue titled "Cua Driver canonical installer canary is failing"' in stdout
    assert 'would open or update the issue titled "Branded installer endpoint check is failing"' in stdout
    assert "x returned HTTP 500" in stdout
