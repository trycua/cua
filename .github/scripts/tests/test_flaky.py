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


# --- job quarantine (ci-gate) and the nightly flake scan ------------------------------


def job(**kw):
    base = {"job": "linux-kvm (concurrent)", "workflow": "Nightly: *", "signature": "Not authorized",
            "owner": "francesco", "issue": "CUA-1", "expires": "2026-11-01"}
    base.update(kw)
    return base


def test_validate_job_entries():
    assert flaky.check_entries([job()], TODAY) == ([], [])
    assert flaky.check_entries([job(signature="")], TODAY)[0]  # a job entry must name its failure
    assert flaky.check_entries([job(signature="(")], TODAY)[0]
    assert flaky.check_entries([job(test="x")], TODAY)[0]


def test_the_repo_quarantine_list_is_valid():
    entries = json.loads(SCRIPT.parents[2].joinpath("flaky", "quarantine.json").read_text())["entries"]
    errors, _ = flaky.check_entries(entries, dt.date.fromisoformat(min(e["expires"] for e in entries)))
    assert errors == []


def test_job_entries_never_match_tests():
    assert flaky.is_quarantined("linux-kvm (concurrent)", "s", [job()]) is None


def gate_check(env, monkeypatch, entries, stdin, logs, workflow="Nightly: VM Space ready after boot"):
    flaky.QUARANTINE_FILE.write_text(json.dumps({"entries": entries}))
    monkeypatch.setattr(sys, "stdin", __import__("io").StringIO(stdin))
    args = flaky.argparse.Namespace(workflow=workflow, quarantine=None, today="2026-10-25")
    out = __import__("io").StringIO()
    monkeypatch.setattr(sys, "stdout", out)
    assert flaky.cmd_gate_check(args, fetch_log=lambda repo, job_id: logs.get(job_id, "")) == 0
    return out.getvalue().strip()


@pytest.mark.parametrize(("stdin", "logs", "workflow", "want"), [
    ("1\tlinux-kvm (concurrent)\n", {"1": "x registry: Not authorized y"}, "Nightly: VM Space ready after boot", "all"),
    # the job's log must show the known failure; a new failure in the same job still blocks
    ("1\tlinux-kvm (concurrent)\n", {"1": "assertion failed"}, "Nightly: VM Space ready after boot", "none"),
    # every failed job must be quarantined
    ("1\tlinux-kvm (concurrent)\n2\tlinux-kvm (sequential)\n", {"1": "Not authorized", "2": "Not authorized"},
     "Nightly: VM Space ready after boot", "none"),
    ("1\tlinux-kvm (concurrent)\n", {"1": "Not authorized"}, "CI: other", "none"),
    ("", {}, "Nightly: VM Space ready after boot", "none"),
])
def test_gate_check(env, monkeypatch, stdin, logs, workflow, want):
    assert gate_check(env, monkeypatch, [job()], stdin, logs, workflow) == want


def test_gate_check_ignores_expired_entries(env, monkeypatch):
    assert gate_check(env, monkeypatch, [job(expires="2026-10-01")], "1\tlinux-kvm (concurrent)\n",
                      {"1": "Not authorized"}) == "none"


def test_detect_reports_jobs_that_failed_then_passed_on_a_rerun(env, monkeypatch):
    monkeypatch.setenv("GITHUB_REPOSITORY", "o/r")
    pages = {
        "runs?": {"workflow_runs": [
            {"id": 7, "name": "CI: jev-use", "run_attempt": 2, "head_sha": "abc", "head_branch": "b",
             "pull_requests": [{"number": 5}]},
            {"id": 8, "name": "CI: once", "run_attempt": 1},
        ]},
        "runs/7/attempts/1/": {"jobs": [{"name": "macOS mock E2E", "conclusion": "failure"},
                                        {"name": "lint", "conclusion": "success"}]},
        "runs/7/attempts/2/": {"jobs": [{"name": "macOS mock E2E", "conclusion": "success"}]},
    }
    api = lambda path: next(v for k, v in pages.items() if k in path)
    args = flaky.argparse.Namespace(out=str(env / "out"), since_hours=24, max_pages=2)
    assert flaky.cmd_detect(args, api=api) == 0
    report = json.loads((env / "out" / "run-7.json").read_text())
    assert report["kind"] == "job" and report["pr"] == 5
    assert [r["id"] for r in report["flaky"]] == ["CI: jev-use / macOS mock E2E"]
    assert not (env / "out" / "run-8.json").exists()


def test_digest_counts_retries_per_test_and_sorts(env, monkeypatch):
    monkeypatch.setenv("GITHUB_REPOSITORY", "o/r")
    reports = [
        {"run_id": "1", "run_attempt": "1", "flaky": [{"id": "a::b"}, {"id": "c::d"}]},
        {"run_id": "2", "run_attempt": "1", "flaky": [{"id": "c::d"}]},
        {"run_id": "2", "run_attempt": "1", "flaky": [{"id": "c::d"}]},  # same run twice counts once
    ]
    body = flaky.build_digest(reports, "2026-11-01")
    assert body.index("c::d") < body.index("a::b")
    assert "| 2 | `c::d` |" in body and "| 1 | `a::b` |" in body
    args = flaky.argparse.Namespace(days=7, dry_run=True)
    assert flaky.cmd_digest(args, list_artifacts=lambda r, s: [{"id": 1}], fetch=lambda r, a: reports) == 0


def test_record_writes_summary_only(env, capsys):
    d = env / "res"
    d.mkdir()
    (d / "x.json").write_text(json.dumps({"suite": "s", "run_id": "9", "run_attempt": "2", "flaky": [{"id": "t::x"}]}))
    assert flaky.main(["record", "--dir", str(d)]) == 0
    assert "t::x" in (env / "summary.md").read_text()
