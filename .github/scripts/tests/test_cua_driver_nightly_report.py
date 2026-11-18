"""Execute the nightly issue reporter with local evidence and a fake GitHub CLI."""

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]


def step_script(job, name):
    workflow = yaml.safe_load(
        (ROOT / ".github/workflows/e2e-cua-driver-release-gate.yml").read_text()
    )
    return next(step["run"] for step in workflow["jobs"][job]["steps"] if step.get("name") == name)


def full_run_lanes(overrides=None):
    lanes = {
        name: {"result": "success"} for name in ("decide", "linux", "windows", "macos", "browsers")
    }
    # quick and macos-pr run only on PR events, so a full run reports them skipped.
    lanes["quick"] = {"result": "skipped"}
    lanes["macos-pr"] = {"result": "skipped"}
    for name, result in (overrides or {}).items():
        lanes[name]["result"] = result
    return lanes


def run_report(tmp_path, *, overrides=None, existing="", flaky=False):
    script = step_script("nightly-report", "Open, update or close the tracking issue")
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
    lanes = full_run_lanes(overrides)
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


def run_result(tmp_path, *, event="push", overrides=None):
    script = step_script("result", "Evaluate lanes")
    summary_path = tmp_path / "result-summary"
    return subprocess.run(
        ["bash", "-c", script],
        env={
            "PATH": os.environ["PATH"],
            "RUN": "true",
            "LANES_SIZE": "full",
            "EVENT": event,
            "DECIDE": "success",
            "LANES": json.dumps(full_run_lanes(overrides)),
            "GITHUB_STEP_SUMMARY": str(summary_path),
        },
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.parametrize("event", ["push", "schedule", "workflow_dispatch"])
def test_full_run_result_passes_when_only_pr_jobs_skipped(tmp_path, event):
    result = run_result(tmp_path, event=event)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "All release-gate lanes passed." in result.stdout


@pytest.mark.parametrize("lane", ["linux", "windows", "macos", "browsers"])
def test_full_run_result_fails_on_a_skipped_lane(tmp_path, lane):
    result = run_result(tmp_path, overrides={lane: "skipped"})

    assert result.returncode == 1
    assert f"{lane}=skipped" in result.stdout
