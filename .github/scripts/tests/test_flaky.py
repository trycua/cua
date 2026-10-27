import datetime as dt
import importlib.util
import json
import sys
import textwrap
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "flaky" / "flaky.py"
spec = importlib.util.spec_from_file_location("flaky", SCRIPT)
flaky = importlib.util.module_from_spec(spec)
sys.modules["flaky"] = flaky
spec.loader.exec_module(flaky)

TODAY = dt.date(2026, 10, 25)
JUNIT = textwrap.dedent(
    """\
    <testsuites><testsuite name="pkg::bin">
      <testcase classname="pkg::bin" name="ok"/>
      <testcase classname="pkg::bin" name="racy"><flakyFailure message="timed out"/></testcase>
      <testcase classname="pkg::bin" name="broken"><failure message="boom"/><rerunFailure message="boom"/></testcase>
      <testcase classname="pkg::bin" name="skip"><skipped/></testcase>
    </testsuite></testsuites>
    """
)


def entry(**kw):
    base = {"test": "pkg::bin::broken", "owner": "francesco", "issue": "CUA-1", "expires": "2026-11-01"}
    base.update(kw)
    return base


@pytest.fixture
def env(tmp_path, monkeypatch):
    quarantine = tmp_path / "quarantine.json"
    quarantine.write_text(json.dumps({"entries": []}))
    monkeypatch.setattr(flaky, "QUARANTINE_FILE", quarantine)
    monkeypatch.setenv("RUNNER_TEMP", str(tmp_path))
    monkeypatch.setenv("GITHUB_STEP_SUMMARY", str(tmp_path / "summary.md"))
    monkeypatch.setattr(flaky.dt, "date", type("D", (dt.date,), {"today": staticmethod(lambda: TODAY)}))
    return tmp_path


def run(env, *extra, exit_code=1, junit=JUNIT):
    """Run a fake test command that writes `junit` and exits with `exit_code`."""
    out = env / "junit.xml"
    script = f"import pathlib; pathlib.Path({str(out)!r}).write_text({junit!r}); raise SystemExit({exit_code})"
    return flaky.main(["run", "--suite", "t", "--format", "junit", "--results", str(out), *extra, "--",
                       sys.executable, "-c", script])


def test_parse_junit_statuses(env):
    (env / "junit.xml").write_text(JUNIT)
    got = flaky.parse_junit([str(env / "junit.xml")])
    assert {k: v["status"] for k, v in got.items()} == {
        "pkg::bin::ok": "passed", "pkg::bin::racy": "flaky", "pkg::bin::broken": "failed", "pkg::bin::skip": "skipped"}


def test_real_failure_still_fails_and_flaky_is_reported(env):
    assert run(env) == 1
    assert "pkg::bin::racy" in (env / "summary.md").read_text()
    report = json.loads((env / "flaky-results" / "t.json").read_text())
    assert [row["id"] for row in report["flaky"]] == ["pkg::bin::racy"]


def test_flaky_only_passes_with_report(env):
    assert run(env, exit_code=0, junit=JUNIT.replace('<failure message="boom"/><rerunFailure message="boom"/>', "")) == 0
    assert (env / "flaky-results" / "t.json").exists()


def test_quarantined_failure_does_not_block(env):
    flaky.QUARANTINE_FILE.write_text(json.dumps({"entries": [entry()]}))
    assert run(env) == 0


def test_expired_quarantine_blocks(env):
    flaky.QUARANTINE_FILE.write_text(json.dumps({"entries": [entry(expires="2026-10-01")]}))
    assert run(env) == 1


def test_quarantine_does_not_hide_other_failures(env):
    flaky.QUARANTINE_FILE.write_text(json.dumps({"entries": [entry(test="pkg::bin::other")]}))
    assert run(env) == 1


def test_failure_without_parsed_tests_fails(env):
    flaky.QUARANTINE_FILE.write_text(json.dumps({"entries": [entry()]}))
    assert run(env, junit="<testsuites/>") == 1


def test_validate(env):
    assert flaky.check_entries([entry()], TODAY) == ([], [])
    errors, expired = flaky.check_entries([entry(owner="", issue="soon"), entry(expires="2026-10-24")], TODAY)
    assert len(errors) == 2 and len(expired) == 1
    assert flaky.check_entries([entry(expires="2027-06-01")], TODAY)[0]
    assert flaky.main(["validate", "--today", "2026-10-25"]) == 0
    flaky.QUARANTINE_FILE.write_text(json.dumps({"entries": [entry(expires="2026-10-01")]}))
    assert flaky.main(["validate", "--today", "2026-10-25"]) == 1


def test_parse_log_swift_and_xcodebuild():
    lines = [
        "✘ Test racy() failed after 0.001 seconds with 1 issue.",
        "✔ Test racy() passed after 0.001 seconds.",
        "✘ Test broken() failed after 0.001 seconds with 1 issue.",
        "✘ Test run with 3 tests in 1 suite failed after 0.001 seconds with 2 issues.",
        "Test Case '-[LTests.X testY]' failed (0.1 seconds).",
        "Test case 'UITests.go()' failed on 'My Mac - App (1)' (1.0 seconds)",
        "Test case 'UITests.go()' passed on 'My Mac - App (1)' (1.0 seconds)",
    ]
    got = {k: v["status"] for k, v in flaky.parse_log(lines).items()}
    assert got == {"racy()": "flaky", "broken()": "failed", "LTests.X/testY": "failed", "UITests.go()": "flaky"}


def test_parse_playwright(tmp_path):
    flows = [
        {"title": "one", "tests": [{"projectName": "chromium", "status": "flaky",
                                    "results": [{"error": {"message": "\x1b[31mx\x1b[0m"}}]}]},
        {"title": "two", "tests": [{"projectName": "chromium", "status": "unexpected", "results": []}]},
    ]
    report = {"suites": [{"title": "a.spec.ts", "specs": [], "suites": [{"title": "flows", "specs": flows}]}]}
    path = tmp_path / "pw.json"
    path.write_text(json.dumps(report))
    got = flaky.parse_playwright([str(path)])
    assert got["a.spec.ts > flows > one [chromium]"] == {"status": "flaky", "message": "x"}
    assert got["a.spec.ts > flows > two [chromium]"]["status"] == "failed"
