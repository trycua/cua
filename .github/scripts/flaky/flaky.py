#!/usr/bin/env python3
"""Flaky-test handling for CI: report, quarantine, and file issues.

The test runners retry once themselves (cargo nextest `retries = 1`, vitest
`retry`, Playwright `retries`, xcodebuild `-retry-tests-on-failure`). This tool
wraps the runner and reads its result file:

  run       run a test command, then read its results. A test that passed only
            on the retry is reported (job summary, annotation, JSON artifact),
            never hidden. A failure that is not in the quarantine list still
            fails the job. If every failing test is in the quarantine list and
            not expired, the job passes with a warning.
  validate  check .github/flaky/quarantine.json; exit 1 on an invalid or
            expired entry.
  record     write the flaky results of one run to the job summary. It never
            comments or opens issues.
  digest    weekly: open or update ONE issue, "Flaky tests this week", with a
            retry count per test or job from the last 7 days of `flaky-*`
            artifacts.
  gate-check
            for ci-gate: read the failed jobs of one workflow run on stdin
            ("<job id>\t<job name>" lines) and print `all` when every one
            matches an active job entry whose `signature` is in its log.
            ci-gate then reruns the failed jobs once; a quarantined job that
            fails again is reported and does not fail the gate.
  detect    for the nightly flake scan: find jobs that failed and then passed
            on a rerun of the same commit, and write reports for `record` and `digest`.

Quarantine entries (.github/flaky/quarantine.json) name either a `test` (a
test id, matched inside a job by `run`) or a `job` (a check name glob, with an
optional `workflow` name glob, matched by ci-gate). A `job` entry must also
carry a `signature`: a regex that must appear in the failed job's log, so only
the known failure is forgiven, never a new one in the same job.

Result formats (--format): junit (cargo nextest), vitest (the reporter in
.github/flaky/vitest-reporter.mjs), playwright (the JSON reporter), log (swift
test and xcodebuild console output).
"""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import glob
import io
import json
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Tuple

QUARANTINE_FILE = Path(__file__).resolve().parents[2] / "flaky" / "quarantine.json"
MAX_QUARANTINE_DAYS = 90
ISSUE_RE = re.compile(r"^(CUA-\d+|#\d+|https://github\.com/[\w.-]+/[\w.-]+/issues/\d+|https://linear\.app/\S+)$")
RANK = {"skipped": 0, "passed": 1, "flaky": 2, "failed": 3}

Results = Dict[str, dict]


def merge(into: Results, test_id: str, status: str, message: str = "") -> None:
    old = into.get(test_id)
    if old is None or RANK[status] > RANK[old["status"]]:
        into[test_id] = {"status": status, "message": message}


# --- parsers ---------------------------------------------------------------


def parse_junit(paths: Iterable[str]) -> Results:
    out: Results = {}
    for path in paths:
        for case in ET.parse(path).getroot().iter("testcase"):
            cls, name = case.get("classname", ""), case.get("name", "")
            test_id = f"{cls}::{name}" if cls and cls != name else name
            tags = {child.tag: child for child in case}
            if "failure" in tags or "error" in tags:
                node = tags.get("failure", tags.get("error"))
                merge(out, test_id, "failed", (node.get("message") or node.text or "").strip())
            elif "flakyFailure" in tags or "flakyError" in tags:
                node = tags.get("flakyFailure", tags.get("flakyError"))
                merge(out, test_id, "flaky", (node.get("message") or node.text or "").strip())
            elif "skipped" in tags:
                merge(out, test_id, "skipped")
            else:
                merge(out, test_id, "passed")
    return out


def parse_vitest(paths: Iterable[str]) -> Results:
    out: Results = {}
    for path in paths:
        for row in json.loads(Path(path).read_text()):
            merge(out, row["id"], row["status"], row.get("message", ""))
    return out


def parse_playwright(paths: Iterable[str]) -> Results:
    out: Results = {}
    status_map = {"expected": "passed", "unexpected": "failed", "flaky": "flaky", "skipped": "skipped"}

    def walk(suite: dict, trail: List[str]) -> None:
        here = trail + ([suite["title"]] if suite.get("title") else [])
        for spec in suite.get("specs", []):
            for test in spec.get("tests", []):
                project = test.get("projectName", "")
                test_id = " > ".join(here + [spec["title"]]) + (f" [{project}]" if project else "")
                message = ""
                for result in test.get("results", []):
                    err = (result.get("error") or {}).get("message")
                    if err:
                        message = re.sub(r"\x1b\[[0-9;]*m", "", err)
                        break
                merge(out, test_id, status_map.get(test.get("status", ""), "passed"), message)
        for child in suite.get("suites", []):
            walk(child, here)

    for path in paths:
        for suite in json.loads(Path(path).read_text()).get("suites", []):
            walk(suite, [])
    return out


LOG_PATTERNS = [
    # Swift Testing: "✘ Test realFail() failed after 0.001 seconds"
    (re.compile(r"^[✔✘] Test (?!run with )(.+?) (passed|failed) after"), lambda m: m.group(1)),
    # XCTest: "Test Case '-[LTests.X testY]' failed (0.1 seconds)."
    (re.compile(r"^Test Case '-\[(\S+) (\S+)\]' (passed|failed)"), lambda m: f"{m.group(1)}/{m.group(2)}"),
    # xcodebuild: "Test case 'X.y()' failed on 'My Mac - app (123)' (0.1 seconds)"
    (re.compile(r"^Test case '(.+?)' (passed|failed) on "), lambda m: m.group(1)),
]


def parse_log(lines: Iterable[str]) -> Results:
    """A test that failed and later passed in the same log is flaky."""
    seen: Dict[str, List[str]] = {}
    for line in lines:
        line = line.strip()
        for pattern, ident in LOG_PATTERNS:
            m = pattern.match(line)
            if m:
                seen.setdefault(ident(m), []).append(m.group(m.lastindex))
                break
    out: Results = {}
    for test_id, seq in seen.items():
        if seq[-1] == "failed":
            status = "failed"
        elif "failed" in seq:
            status = "flaky"
        else:
            status = "passed"
        out[test_id] = {"status": status, "message": ""}
    return out


# --- quarantine --------------------------------------------------------------


def load_quarantine() -> List[dict]:
    return json.loads(QUARANTINE_FILE.read_text()).get("entries", [])


def check_entries(entries: List[dict], today: dt.date) -> Tuple[List[str], List[str]]:
    """Return (errors, expired). Both make `validate` fail."""
    errors: List[str] = []
    expired: List[str] = []
    for i, entry in enumerate(entries):
        label = entry.get("test") or entry.get("job") or f"entry {i}"
        kind = "job" if "job" in entry else "test"
        if "job" in entry and "test" in entry:
            errors.append(f"{label}: set `test` or `job`, not both")
        for key in (kind, "owner", "issue", "expires"):
            if not str(entry.get(key, "")).strip():
                errors.append(f"{label}: missing `{key}`")
        if kind == "job":
            signature = str(entry.get("signature") or "")
            if not signature.strip():
                errors.append(f"{label}: a `job` entry needs a `signature` regex from the failure log")
            else:
                try:
                    re.compile(signature)
                except re.error as exc:
                    errors.append(f"{label}: `signature` is not a valid regex ({exc})")
        if entry.get("issue") and not ISSUE_RE.match(entry["issue"]):
            errors.append(f"{label}: `issue` must be CUA-123, #123, or an issue URL, got {entry['issue']!r}")
        try:
            day = dt.date.fromisoformat(entry.get("expires", ""))
        except ValueError:
            errors.append(f"{label}: `expires` must be YYYY-MM-DD, got {entry.get('expires')!r}")
            continue
        if day < today:
            expired.append(f"{label}: expired on {day} (owner {entry.get('owner')}, issue {entry.get('issue')})")
        elif (day - today).days > MAX_QUARANTINE_DAYS:
            errors.append(f"{label}: `expires` is more than {MAX_QUARANTINE_DAYS} days away")
    return errors, expired


def active_entries(entries: List[dict], today: dt.date) -> List[dict]:
    good = []
    for entry in entries:
        try:
            if dt.date.fromisoformat(entry["expires"]) >= today and entry.get("owner") and entry.get("issue"):
                good.append(entry)
        except (KeyError, ValueError):
            pass
    return good


def is_quarantined(test_id: str, suite: str, entries: List[dict]) -> Optional[dict]:
    for entry in entries:
        if "test" not in entry:
            continue
        if not (fnmatch.fnmatchcase(test_id, entry["test"]) or test_id.endswith("::" + entry["test"])):
            continue
        if entry.get("suite") and not fnmatch.fnmatchcase(suite, entry["suite"]):
            continue
        return entry
    return None


# --- run -----------------------------------------------------------------


def stream(cmd: List[str]) -> Tuple[int, List[str]]:
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace")
    lines: List[str] = []
    assert proc.stdout is not None
    for line in proc.stdout:
        sys.stdout.write(line)
        lines.append(line)
    return proc.wait(), lines


def run_metadata() -> dict:
    meta = {
        "repo": os.environ.get("GITHUB_REPOSITORY", ""),
        "run_id": os.environ.get("GITHUB_RUN_ID", ""),
        "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT", ""),
        "workflow": os.environ.get("GITHUB_WORKFLOW", ""),
        "job": os.environ.get("GITHUB_JOB", ""),
        "ref": os.environ.get("GITHUB_REF", ""),
        "sha": os.environ.get("GITHUB_SHA", ""),
        "pr": None,
    }
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    if event_path and Path(event_path).is_file():
        pr = json.loads(Path(event_path).read_text()).get("pull_request")
        if pr:
            meta["pr"] = pr.get("number")
            meta["sha"] = pr.get("head", {}).get("sha", meta["sha"])
    return meta


def cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")[:200]


def cmd_run(args: argparse.Namespace) -> int:
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        print("flaky.py run: no command after --", file=sys.stderr)
        return 2
    today = dt.date.today()
    entries = load_quarantine()
    bad, expired = check_entries(entries, today)
    live = active_entries(entries, today)
    for problem in bad + expired:
        print(f"::warning title=Quarantine list::{problem}")

    for stale in (p for pattern in args.results for p in glob.glob(pattern, recursive=True)):
        os.remove(stale)  # a leftover file from an earlier run must not be read as this run's
    code, lines = stream(command)
    paths = sorted(p for pattern in args.results for p in glob.glob(pattern, recursive=True))
    try:
        if args.format == "log":
            results = parse_log(lines)
            failed_first = sorted(t for t, r in results.items() if r["status"] == "failed")
            if code != 0 and failed_first and args.rerun_failed:
                # swift test has no retry flag: run the failures once more.
                rerun_cmd = list(command)
                for test_id in failed_first:
                    rerun_cmd += ["--filter", re.escape(test_id)]
                print(f"::notice title=Retry::re-running {len(failed_first)} failed test(s) once")
                code, lines2 = stream(rerun_cmd)
                second = parse_log(lines2)
                for test_id in failed_first:
                    after = second.get(test_id, {"status": "failed"})["status"]
                    results[test_id]["status"] = "flaky" if after == "passed" else "failed"
        elif args.format == "junit":
            results = parse_junit(paths)
        elif args.format == "vitest":
            results = parse_vitest(paths)
        else:
            results = parse_playwright(paths)
    except (OSError, ValueError, KeyError, ET.ParseError) as exc:
        print(f"::warning title=Flaky report::could not read results ({exc}); exit code {code} stands")
        return code

    flaky = {t: r for t, r in results.items() if r["status"] == "flaky"}
    failed = {t: r for t, r in results.items() if r["status"] == "failed"}
    quarantined = {t: r for t, r in failed.items() if is_quarantined(t, args.suite, live)}
    real = {t: r for t, r in failed.items() if t not in quarantined}

    for test_id, r in flaky.items():
        print(f"::warning title=Flaky test ({args.suite})::{test_id} passed only on retry")
    for test_id in quarantined:
        entry = is_quarantined(test_id, args.suite, live)
        print(
            f"::warning title=Quarantined test failed ({args.suite})::{test_id} "
            f"(owner {entry['owner']}, {entry['issue']}, expires {entry['expires']})"
        )
    for test_id in real:
        print(f"::error title=Test failed ({args.suite})::{test_id}")

    write_summary(args.suite, results, flaky, quarantined, real, live)
    write_artifact(args.suite, flaky, quarantined)
    record_timings(args.suite, args.format, paths, lines)

    if code == 0:
        return 0
    # The runner failed. Pass only if it failed on quarantined tests and nothing else.
    if quarantined and not real:
        print(f"::warning title=Quarantine::{len(quarantined)} quarantined test(s) failed; not blocking")
        return 0
    return code


def write_summary(suite: str, results: Results, flaky: Results, quarantined: Results, real: Results, live: List[dict]) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not path or not (flaky or quarantined or real):
        return
    out = [f"### Test results: {suite}", ""]
    counts = {s: sum(1 for r in results.values() if r["status"] == s) for s in RANK}
    out.append(
        f"{counts['passed']} passed, {len(flaky)} flaky (passed only on retry), "
        f"{len(real)} failed, {len(quarantined)} quarantined failures, {counts['skipped']} skipped."
    )
    if flaky:
        out += ["", "**Flaky: passed only on retry. A `flaky` issue is opened or updated for each.**", "", "| Test | First failure |", "|---|---|"]
        out += [f"| `{cell(t)}` | {cell(r['message'])} |" for t, r in sorted(flaky.items())]
    if quarantined:
        out += ["", "**Quarantined: failed but not blocking.**", "", "| Test | Owner | Issue | Expires |", "|---|---|---|---|"]
        for t in sorted(quarantined):
            e = is_quarantined(t, suite, live)
            out.append(f"| `{cell(t)}` | {e['owner']} | {e['issue']} | {e['expires']} |")
    if real:
        out += ["", "**Failed (blocking).**", "", "| Test | Failure |", "|---|---|"]
        out += [f"| `{cell(t)}` | {cell(r['message'])} |" for t, r in sorted(real.items())]
    with open(path, "a") as fh:
        fh.write("\n".join(out) + "\n")


def record_timings(suite: str, fmt: str, paths: List[str], lines: List[str]) -> None:
    """Per-file test times (timings.py). Measurement only: never changes the verdict."""
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import timings

        if fmt == "junit":
            files = timings.junit_files(paths)
        elif fmt == "log":
            files = timings.swift_log_suites(lines)
        elif fmt == "vitest":
            files = []
            for path in paths:
                side = Path(path + ".timings.json")
                if side.is_file():
                    files += json.loads(side.read_text())
        else:
            return
        timings.write(suite, files)
    except Exception as exc:  # noqa: BLE001
        print(f"::warning title=Test timings::{suite}: {exc}")


def write_artifact(suite: str, flaky: Results, quarantined: Results) -> None:
    base = os.environ.get("RUNNER_TEMP")
    if not base or not (flaky or quarantined):
        return
    out_dir = Path(base) / "flaky-results"
    out_dir.mkdir(parents=True, exist_ok=True)
    payload = dict(run_metadata(), suite=suite, flaky=[dict(id=t, message=r["message"][:1500]) for t, r in flaky.items()],
                   quarantined=sorted(quarantined))
    (out_dir / f"{re.sub(r'[^A-Za-z0-9_.-]', '_', suite)}.json").write_text(json.dumps(payload, indent=2))


# --- validate ----------------------------------------------------------------


def cmd_validate(args: argparse.Namespace) -> int:
    today = dt.date.fromisoformat(args.today) if args.today else dt.date.today()
    try:
        entries = load_quarantine()
    except (OSError, ValueError) as exc:
        print(f"::error::cannot read {QUARANTINE_FILE}: {exc}")
        return 1
    errors, expired = check_entries(entries, today)
    for problem in errors:
        print(f"::error title=Quarantine entry invalid::{problem}")
    for problem in expired:
        print(f"::error title=Quarantine entry expired::{problem}. Fix the test and remove the entry, or renew it with a reason.")
    print(f"{len(entries)} quarantine entr{'y' if len(entries) == 1 else 'ies'}, {len(errors)} invalid, {len(expired)} expired")
    return 1 if errors or expired else 0


# --- issues ----------------------------------------------------------------


def clean(text: str, limit: int) -> str:
    text = re.sub(r"[\x00-\x08\x0b-\x1f\x7f]", "", text)
    return text.replace("@", "@\u200b")[:limit]


def gh(*args: str, check: bool = True) -> str:
    done = subprocess.run(["gh", *args], capture_output=True, text=True)
    if check and done.returncode != 0:
        raise RuntimeError(f"gh {' '.join(args[:3])} failed: {done.stderr.strip()}")
    return done.stdout


def load_reports(directory: str) -> List[dict]:
    return [json.loads(Path(p).read_text()) for p in sorted(glob.glob(f"{directory}/**/*.json", recursive=True))]


def cmd_record(args: argparse.Namespace) -> int:
    """Write the flaky results of one run to the job summary. Never comments or opens issues:
    `digest` turns the artifacts into one weekly issue."""
    rows = [(r, row) for r in load_reports(args.dir) for row in r.get("flaky", [])]
    if not rows:
        print("no flaky tests reported")
        return 0
    lines = ["### Passed only on retry", "", "| Test or job | Suite | Run |", "|---|---|---|"]
    for report, row in rows:
        lines.append(f"| `{cell(clean(row['id'], 200))}` | {cell(clean(report['suite'], 80))} | {report['run_id']} (attempt {report['run_attempt']}) |")
    text = "\n".join(lines) + "\n"
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as fh:
            fh.write(text)
    return 0


DIGEST_TITLE = "Flaky tests this week"
DIGEST_MARKER = "<!-- flaky-digest -->"


def build_digest(reports: List[dict], since: str) -> str:
    """Retry count per test or job: one per distinct (run, attempt) it passed only on retry in."""
    seen: Dict[str, set] = {}
    for report in reports:
        for row in report.get("flaky", []):
            seen.setdefault(row["id"], set()).add((report.get("run_id"), report.get("run_attempt")))
    ranked = sorted(seen.items(), key=lambda kv: (-len(kv[1]), kv[0]))
    lines = [DIGEST_MARKER, f"Tests and jobs that passed only on retry since {since} (UTC). "
             "Updated weekly. Fix the cause, or quarantine in `.github/flaky/quarantine.json` "
             "with an owner, an issue and an expiry date.", ""]
    if not ranked:
        lines.append("None this week.")
    else:
        lines += ["| Retries | Test or job |", "|---|---|"]
        lines += [f"| {len(runs)} | `{cell(clean(test_id, 200))}` |" for test_id, runs in ranked]
    return "\n".join(lines) + "\n"


def list_flaky_artifacts(repo: str, since: dt.datetime) -> List[dict]:
    found, page = [], 1
    while page <= 30:
        data = json.loads(gh("api", f"repos/{repo}/actions/artifacts?per_page=100&page={page}"))
        batch = data.get("artifacts", [])
        for art in batch:
            created = dt.datetime.fromisoformat(art["created_at"].replace("Z", "+00:00"))
            if created >= since and not art.get("expired") and art["name"].startswith("flaky-"):
                found.append(art)
        if len(batch) < 100 or (batch and dt.datetime.fromisoformat(batch[-1]["created_at"].replace("Z", "+00:00")) < since):
            break
        page += 1
    return found


def fetch_artifact_reports(repo: str, art: dict) -> List[dict]:
    done = subprocess.run(["gh", "api", f"repos/{repo}/actions/artifacts/{art['id']}/zip"], capture_output=True)
    if done.returncode != 0:
        return []
    out = []
    with zipfile.ZipFile(io.BytesIO(done.stdout)) as z:
        for name in z.namelist():
            if name.endswith(".json"):
                try:
                    out.append(json.loads(z.read(name)))
                except ValueError:
                    pass
    return out


def cmd_digest(args: argparse.Namespace, list_artifacts=list_flaky_artifacts, fetch=fetch_artifact_reports) -> int:
    """Open or update ONE issue, "Flaky tests this week", from the last 7 days of flaky-* artifacts."""
    repo = os.environ["GITHUB_REPOSITORY"]
    since = dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=args.days)
    reports = [r for art in list_artifacts(repo, since) for r in fetch(repo, art)]
    body = build_digest(reports, since.strftime("%Y-%m-%d"))
    print(body)
    if args.dry_run:
        return 0
    gh("label", "create", "flaky", "--repo", repo, "--color", "FBCA04", "--description", "Test that passed only on retry", check=False)
    existing = json.loads(gh("issue", "list", "--repo", repo, "--label", "flaky", "--state", "open", "--limit", "200",
                             "--json", "number,title,body") or "[]")
    issue = next((i for i in existing if DIGEST_MARKER in (i.get("body") or "")), None)
    if issue:
        gh("issue", "edit", str(issue["number"]), "--repo", repo, "--body", body)
        print(f"updated #{issue['number']}")
    else:
        print(gh("issue", "create", "--repo", repo, "--label", "flaky", "--title", DIGEST_TITLE, "--body", body).strip())
    return 0


# --- gate-check (ci-gate) -------------------------------------------------------


def job_entry(workflow: str, job: str, entries: List[dict]) -> Optional[dict]:
    for entry in entries:
        if "job" not in entry or not entry.get("signature"):
            continue
        if not fnmatch.fnmatchcase(job, entry["job"]):
            continue
        if entry.get("workflow") and not fnmatch.fnmatchcase(workflow, entry["workflow"]):
            continue
        return entry
    return None


def job_log(repo: str, job_id: str) -> str:
    done = subprocess.run(["gh", "api", f"repos/{repo}/actions/jobs/{job_id}/logs"],
                          capture_output=True, text=True, errors="replace")
    return done.stdout if done.returncode == 0 else ""


def cmd_gate_check(args: argparse.Namespace, fetch_log=job_log) -> int:
    today = dt.date.fromisoformat(args.today) if args.today else dt.date.today()
    path = Path(args.quarantine) if args.quarantine else QUARANTINE_FILE
    try:
        live = active_entries(json.loads(path.read_text()).get("entries", []), today)
    except (OSError, ValueError):
        live = []
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    verdicts = []
    for line in sys.stdin:
        if not line.strip():
            continue
        job_id, _, name = line.rstrip("\n").partition("\t")
        entry = job_entry(args.workflow, name, live)
        hit = entry is not None and re.search(entry["signature"], fetch_log(repo, job_id)) is not None
        verdicts.append((name, entry if hit else None))
    for name, entry in verdicts:
        if entry:
            print(f"::warning title=Quarantined job ({args.workflow})::{name} failed with its known flake "
                  f"(owner {entry['owner']}, {entry['issue']}, expires {entry['expires']})", file=sys.stderr)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary and any(e for _, e in verdicts):
        with open(summary, "a") as fh:
            fh.write(f"\n**Quarantined flakes in {cell(args.workflow)}**\n\n| Job | Issue | Expires |\n|---|---|---|\n")
            fh.writelines(f"| `{cell(n)}` | {e['issue']} | {e['expires']} |\n" for n, e in verdicts if e)
    print("all" if verdicts and all(e for _, e in verdicts) else "none")
    return 0


# --- detect (nightly) -------------------------------------------------------------


def gh_json(path: str) -> dict:
    return json.loads(gh("api", path))


def cmd_detect(args: argparse.Namespace, api=gh_json) -> int:
    """Jobs that failed and then passed on a rerun of the same commit are flaky."""
    repo = os.environ["GITHUB_REPOSITORY"]
    since = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=args.since_hours)).strftime("%Y-%m-%dT%H:%M:%SZ")
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    found = 0
    page = 1
    while True:
        data = api(f"repos/{repo}/actions/runs?status=success&created=%3E%3D{since}&per_page=100&page={page}")
        runs = data.get("workflow_runs", [])
        for run in runs:
            attempt = int(run.get("run_attempt") or 1)
            if attempt < 2:
                continue
            before = api(f"repos/{repo}/actions/runs/{run['id']}/attempts/{attempt - 1}/jobs?per_page=100")
            failed = {j["name"] for j in before.get("jobs", []) if j.get("conclusion") in ("failure", "timed_out")}
            if not failed:
                continue
            after = api(f"repos/{repo}/actions/runs/{run['id']}/attempts/{attempt}/jobs?per_page=100")
            passed = {j["name"] for j in after.get("jobs", []) if j.get("conclusion") == "success"}
            flaky_jobs = sorted(failed & passed)
            if not flaky_jobs:
                continue
            prs = run.get("pull_requests") or []
            name = run.get("name", "")
            report = {
                "repo": repo, "run_id": str(run["id"]), "run_attempt": str(attempt), "workflow": name,
                "job": ", ".join(flaky_jobs), "ref": run.get("head_branch") or "", "sha": run.get("head_sha", ""),
                "pr": prs[0]["number"] if prs else None, "suite": name, "kind": "job",
                "flaky": [dict(id=f"{name} / {j}",
                               message=f"failed on attempt {attempt - 1}, passed on attempt {attempt} of the same commit")
                          for j in flaky_jobs],
                "quarantined": [],
            }
            (out_dir / f"run-{run['id']}.json").write_text(json.dumps(report, indent=2))
            found += len(flaky_jobs)
            print(f"{name}: {', '.join(flaky_jobs)} (run {run['id']}, attempt {attempt})")
        if len(runs) < 100 or page >= args.max_pages:
            break
        page += 1
    print(f"{found} flaky job(s) since {since}")
    return 0


def main(argv: Optional[List[str]] = None) -> int:
    # Windows runners default stdout to cp1252; test output can contain other characters.
    for stream_ in (sys.stdout, sys.stderr):
        if hasattr(stream_, "reconfigure"):
            stream_.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run")
    run.add_argument("--suite", required=True)
    run.add_argument("--format", choices=["junit", "vitest", "playwright", "log"], required=True)
    run.add_argument("--results", action="append", default=[], help="result file glob (repeatable)")
    run.add_argument("--rerun-failed", action="store_true", help="log format: run the failed tests once more with --filter")
    run.add_argument("command", nargs=argparse.REMAINDER)
    validate = sub.add_parser("validate")
    validate.add_argument("--today")
    record = sub.add_parser("record")
    record.add_argument("--dir", required=True)
    digest = sub.add_parser("digest")
    digest.add_argument("--days", type=int, default=7)
    digest.add_argument("--dry-run", action="store_true")
    gate = sub.add_parser("gate-check")
    gate.add_argument("--workflow", required=True)
    gate.add_argument("--quarantine", help="quarantine file (ci-gate passes the base commit's copy)")
    gate.add_argument("--today")
    detect = sub.add_parser("detect")
    detect.add_argument("--out", required=True)
    detect.add_argument("--since-hours", type=int, default=26)
    detect.add_argument("--max-pages", type=int, default=200)
    args = parser.parse_args(argv)
    return {"run": cmd_run, "validate": cmd_validate, "record": cmd_record, "digest": cmd_digest,
            "gate-check": cmd_gate_check, "detect": cmd_detect}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
