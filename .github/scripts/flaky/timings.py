#!/usr/bin/env python3
"""Per-file test times for CI (CUA-3558, cut #12).

The CI logs did not say which test files are slow, so the next round of cuts
could not be ranked. This reads a runner's own output and writes, per test
file (or suite), the seconds it took:

  * JUnit XML: pytest (`--junitxml`) and cargo nextest. A file is the `file`
    attribute, else the dotted `classname` without its test class.
  * XCTest logs (`swift test`, `xcodebuild test`): one row per test suite,
    from "Test Suite 'X' passed ... Executed N tests ... in S (T) seconds".
  * vitest: the reporter in .github/flaky/vitest-reporter.mjs writes the rows.

Output: a "Slowest test files" table in the job summary and
`$RUNNER_TEMP/ci-timings/<suite>.json`, which `.github/actions/ci-timings-upload`
uploads (artifact `timings-<suite>`); `ci_weekly_report.py` ranks them weekly.

  timings.py junit --suite NAME FILE...        pytest / nextest JUnit files
  timings.py swift-log --suite NAME LOGFILE    an XCTest console log
  flaky.py run calls the same functions for the suites it wraps.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path
from typing import Dict, Iterable, List

TOP = 15
SUITE_RE = re.compile(r"Test Suite '([^']+)' (?:passed|failed) at ")
EXECUTED_RE = re.compile(r"Executed (\d+) tests?, with \d+ failures?.* in ([0-9.]+) \([0-9.]+\) seconds")


def _file_of(case: ET.Element) -> str:
    f = case.get("file")
    if f:
        return f
    cls = case.get("classname", "") or "(no class)"
    parts = cls.split(".")
    # pytest: tests.test_x.TestThing -> tests.test_x
    while len(parts) > 1 and parts[-1][:1].isupper():
        parts.pop()
    return ".".join(parts)


def junit_files(paths: Iterable[str]) -> List[dict]:
    secs: Dict[str, float] = defaultdict(float)
    tests: Dict[str, int] = defaultdict(int)
    for path in paths:
        for case in ET.parse(path).getroot().iter("testcase"):
            key = _file_of(case)
            secs[key] += float(case.get("time") or 0)
            tests[key] += 1
    return rows(secs, tests)


def swift_log_suites(lines: Iterable[str]) -> List[dict]:
    secs: Dict[str, float] = defaultdict(float)
    tests: Dict[str, int] = defaultdict(int)
    suite = None
    for line in lines:
        m = SUITE_RE.search(line)
        if m:
            suite = m.group(1)
            continue
        m = EXECUTED_RE.search(line)
        if m and suite:
            # Skip the bundle and selection totals; keep one row per test class.
            if not suite.endswith((".xctest", "tests")) and suite not in ("All tests", "Selected tests"):
                secs[suite] += float(m.group(2))
                tests[suite] += int(m.group(1))
            suite = None
    return rows(secs, tests)


def rows(secs: Dict[str, float], tests: Dict[str, int]) -> List[dict]:
    return [{"file": k, "seconds": round(v, 3), "tests": tests.get(k, 0)}
            for k, v in sorted(secs.items(), key=lambda kv: -kv[1])]


def write(suite: str, files: List[dict]) -> None:
    """Job summary table and the JSON that the weekly report reads."""
    if not files:
        return
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        out = [f"### Slowest test files: {suite}", "", "| File | Seconds | Tests |", "|---|---:|---:|"]
        out += [f"| `{f['file'].replace('|', '/')}` | {f['seconds']:.1f} | {f['tests']} |" for f in files[:TOP]]
        total = sum(f["seconds"] for f in files)
        out += ["", f"{len(files)} files, {total:.0f} s of test time in total."]
        with open(summary, "a") as fh:
            fh.write("\n".join(out) + "\n")
    base = os.environ.get("RUNNER_TEMP")
    if base:
        out_dir = Path(base) / "ci-timings"
        out_dir.mkdir(parents=True, exist_ok=True)
        meta = {k: os.environ.get(e, "") for k, e in (
            ("repo", "GITHUB_REPOSITORY"), ("run_id", "GITHUB_RUN_ID"), ("workflow", "GITHUB_WORKFLOW"),
            ("job", "GITHUB_JOB"), ("sha", "GITHUB_SHA"))}
        name = re.sub(r"[^A-Za-z0-9_.-]", "_", suite)
        (out_dir / f"{name}.json").write_text(json.dumps(dict(meta, suite=suite, files=files[:200])))


def main(argv: List[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    j = sub.add_parser("junit")
    j.add_argument("--suite", required=True)
    j.add_argument("files", nargs="+")
    s = sub.add_parser("swift-log")
    s.add_argument("--suite", required=True)
    s.add_argument("log")
    args = p.parse_args(argv)
    try:
        if args.cmd == "junit":
            write(args.suite, junit_files(args.files))
        else:
            write(args.suite, swift_log_suites(Path(args.log).read_text(errors="replace").splitlines()))
    except (OSError, ValueError, ET.ParseError) as exc:
        # Timings are measurement only: never fail a job over them.
        print(f"::warning title=Test timings::could not read results for {args.suite}: {exc}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
