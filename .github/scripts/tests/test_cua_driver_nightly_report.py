"""Execute the nightly issue reporter with local evidence and a fake GitHub CLI."""

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]


def run_report(tmp_path, *, overrides=None, existing="", flaky=False):
    workflow = yaml.safe_load(
        (ROOT / ".github/workflows/e2e-cua-driver-release-gate.yml").read_text()
    )
    script = next(
        step["run"]
        for step in workflow["jobs"]["nightly-report"]["steps"]
        if step.get("name") == "Open, update or close the tracking issue"
    )
    scripts = tmp_path / "scripts/ci"
    scripts.mkdir(parents=True)
    shutil.copy2(ROOT / "scripts/ci/summarize-flaky-cases.sh", scripts)
    evidence = tmp_path / "lane-evidence"
    evidence.mkdir()
    if flaky:
        (evidence / "flaky-cases.jsonl").write_text(
            json.dumps({"platform": "linux", "case": "example", "first_attempt_exit": 1}) + "\n"
        )
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    gh = bin_dir / "gh"
    gh.write_text(
        "#!/bin/bash\n"
        'printf "%s\\n" "$*" >> "$GH_CALLS"\n'
        'if [[ "$1 $2" == "issue list" ]]; then\n'
        '  printf "%s" "$EXISTING"\n'
        "fi\n"
    )
    gh.chmod(0o755)
    lanes = {
        name: {"result": "success"} for name in ("decide", "linux", "windows", "macos", "browsers")
    }
    lanes["macos-pr"] = {"result": "skipped"}
    for name, result in (overrides or {}).items():
        lanes[name]["result"] = result
    calls_path = tmp_path / "gh-calls"
    summary_path = tmp_path / "summary"
    result = subprocess.run(
        ["bash", "-c", script],
        cwd=tmp_path,
        env={
            "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
            "LANES": json.dumps(lanes),
            "REPO": "example/repo",
            "SHA": "a" * 40,
            "RUN_URL": "https://example.invalid/run",
            "GITHUB_STEP_SUMMARY": str(summary_path),
            "GH_CALLS": str(calls_path),
            "EXISTING": existing,
        },
        capture_output=True,
        text=True,
        check=False,
    )
    return result, calls_path.read_text(), summary_path.read_text()


@pytest.mark.parametrize("existing", ["", "5267"])
def test_expected_pr_lane_skip_keeps_nightly_green(tmp_path, existing):
    result, calls, summary = run_report(tmp_path, existing=existing)

    assert result.returncode == 0, result.stderr
    assert summary == "Release gate green.\n"
    assert "issue create" not in calls
    assert "issue edit" not in calls
    assert ("issue close 5267" in calls) == bool(existing)


@pytest.mark.parametrize(
    ("lane", "status"),
    [
        (lane, status)
        for lane in ("linux", "windows", "macos", "browsers", "macos-pr")
        for status in ("failure", "cancelled", "skipped")
        if (lane, status) != ("macos-pr", "skipped")
    ],
)
def test_unexpected_lane_results_still_fail(tmp_path, lane, status):
    result, calls, summary = run_report(tmp_path, overrides={lane: status})

    assert result.returncode == 1, result.stderr
    assert f"- `{lane}`: {status}" in summary
    assert "**FAILING**" in summary
    assert "issue create" in calls
    assert "issue close" not in calls


def test_real_failure_updates_existing_tracker(tmp_path):
    result, calls, summary = run_report(tmp_path, overrides={"linux": "failure"}, existing="5267")

    assert result.returncode == 1, result.stderr
    assert "issue edit 5267" in calls
    assert "issue close" not in calls
    assert "- `linux`: failure" in summary


def test_expected_skip_does_not_hide_flaky_evidence(tmp_path):
    result, calls, summary = run_report(tmp_path, flaky=True)

    assert result.returncode == 0, result.stderr
    assert "passing, with flaky cases" in summary
    assert "| linux | `example` | 1 | passed |" in summary
    assert "Failed lanes" not in summary
    assert "issue create" in calls
    assert "issue close" not in calls
