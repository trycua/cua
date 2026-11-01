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
  file-issues
            open or update one GitHub issue per flaky test (label `flaky`)
            from the JSON artifacts that `run` wrote.

Result formats (--format): junit (cargo nextest), vitest (the reporter in
.github/flaky/vitest-reporter.mjs), playwright (the JSON reporter), log (swift
test and xcodebuild console output).
"""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import glob
import hashlib
import json
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
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
        label = entry.get("test", f"entry {i}")
        for key in ("test", "owner", "issue", "expires"):
            if not str(entry.get(key, "")).strip():
                errors.append(f"{label}: missing `{key}`")
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


def flaky_key(test_id: str) -> str:
    return hashlib.sha1(test_id.encode()).hexdigest()[:12]


def cmd_file_issues(args: argparse.Namespace) -> int:
    repo = os.environ["GITHUB_REPOSITORY"]
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
    reports = [json.loads(Path(p).read_text()) for p in sorted(glob.glob(f"{args.dir}/**/*.json", recursive=True))]
    items: Dict[str, List[Tuple[dict, dict]]] = {}
    for report in reports:
        for row in report.get("flaky", []):
            items.setdefault(row["id"], []).append((report, row))
    if not items:
        print("no flaky tests reported")
        return 0
    if args.dry_run:
        for test_id in items:
            print(f"would file: Flaky test: {clean(test_id, 120)}")
        return 0
    gh("label", "create", "flaky", "--repo", repo, "--color", "FBCA04", "--description", "Test that passed only on retry", check=False)
    existing = json.loads(gh("issue", "list", "--repo", repo, "--label", "flaky", "--state", "all", "--limit", "1000",
                             "--json", "number,body,state") or "[]")
    by_key = {}
    for issue in existing:
        m = re.search(r"<!-- flaky-key:(\w+) -->", issue.get("body") or "")
        if m:
            by_key[m.group(1)] = issue
    for test_id, occurrences in items.items():
        key = flaky_key(test_id)
        report, row = occurrences[0]
        run_url = f"{server}/{repo}/actions/runs/{report['run_id']}"
        entry = [
            f"Passed only on retry in `{clean(report['suite'], 80)}`.",
            "",
            f"- Run: {run_url} (attempt {report['run_attempt']}, workflow {clean(report['workflow'], 80)}, job {clean(report['job'], 80)})",
            f"- Commit: `{report['sha'][:12]}`" + (f", PR #{report['pr']}" if report.get("pr") else f", ref `{clean(report['ref'], 80)}`"),
            "",
            "```",
            clean(row.get("message", "") or "(no message)", 1200).replace("```", "'''"),
            "```",
        ]
        issue = by_key.get(key)
        if issue is None:
            body = "\n".join([
                f"`{clean(test_id, 300)}` passes only on retry in CI. Fix the race, or quarantine it in "
                "`.github/flaky/quarantine.json` with an owner, this issue and an expiry date.",
                "",
                "Failure history is in the comments below, one per CI run.",
                "",
                f"<!-- flaky-key:{key} -->",
            ])
            url = gh("issue", "create", "--repo", repo, "--label", "flaky",
                     "--title", f"Flaky test: {clean(test_id, 200)}", "--body", body).strip()
            number = url.rsplit("/", 1)[-1]
            print(f"opened {url}")
        else:
            number = str(issue["number"])
            have = gh("issue", "view", number, "--repo", repo, "--json", "comments", "--jq", ".comments[].body")
            if f"/actions/runs/{report['run_id']}" in have:
                print(f"#{number}: run {report['run_id']} already recorded")
                continue
            if issue["state"] == "CLOSED":
                gh("issue", "reopen", number, "--repo", repo)
        gh("issue", "comment", number, "--repo", repo, "--body", "\n".join(entry))
        print(f"#{number}: recorded {run_url}")
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
    issues = sub.add_parser("file-issues")
    issues.add_argument("--dir", required=True)
    issues.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    return {"run": cmd_run, "validate": cmd_validate, "file-issues": cmd_file_issues}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
