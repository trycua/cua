#!/usr/bin/env python3
"""Open or update ONE issue per workflow that fails on main after merge.

Heavy suites run on main (and at release) instead of on every pull request, so a
failure there lands on a commit that is already merged and is fixed forward. The
alert goes out through workflow-failure-alerts.yml (Alertmanager). This script
adds the tracking issue:

  - first red run: opens "Post-merge CI failing on main: <workflow>" with the label ci-postmerge;
  - later red runs: comment on that same issue (never a second one);
  - a green run: comments and closes it.

Only runs on the default branch from a push or a manual dispatch count; a pull
request or feature-branch run says nothing about main. Standard library only,
talks to GitHub through the gh CLI.

    postmerge_issue.py --workflow "CI: Cua Spaces" --conclusion failure --event push \
        --branch main --default-branch main --sha <sha> --run-url <url> --repo trycua/cua
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

LABEL = "ci-postmerge"
FAILED = {"failure", "timed_out", "startup_failure"}
EVENTS = {"push", "workflow_dispatch"}


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def title_for(workflow: str) -> str:
    return f"Post-merge CI failing on main: {workflow}"


def find_open(workflow: str, repo: str, run=gh) -> int | None:
    rows = json.loads(run("issue", "list", "--repo", repo, "--label", LABEL, "--state", "open",
                          "--limit", "100", "--json", "number,title"))
    want = title_for(workflow)
    for row in rows:
        if row["title"] == want:
            return int(row["number"])
    return None


def act(a, run=gh) -> str:
    """Returns what it did: ignored, opened, commented, closed or nothing."""
    if a.event not in EVENTS or a.branch != a.default_branch:
        return "ignored"
    short = (a.sha or "")[:10]
    existing = find_open(a.workflow, a.repo, run)
    if a.conclusion in FAILED:
        why = {"failure": "a job failed", "timed_out": "the run timed out",
               "startup_failure": "GitHub rejected the run before any job started"}[a.conclusion]
        line = f"`{a.workflow}` failed on main at {short}: {why}. Run: {a.run_url}"
        if existing is None:
            body = (line + "\n\nThis suite runs after merge (and at release) instead of on every pull request, "
                    "so the break is already on main. Fix forward: open a PR against main that repairs it. "
                    "This issue is updated by each later failure and closed by the next green run. "
                    "Alert: Alertmanager, source github-actions.")
            run("label", "create", LABEL, "--repo", a.repo, "--color", "B60205", "--force",
                "--description", "A suite that runs after merge is red on main")
            run("issue", "create", "--repo", a.repo, "--title", title_for(a.workflow), "--label", LABEL, "--body", body)
            return "opened"
        run("issue", "comment", str(existing), "--repo", a.repo, "--body", "Still failing. " + line)
        return "commented"
    if a.conclusion == "success" and existing is not None:
        run("issue", "close", str(existing), "--repo", a.repo, "--comment",
            f"Green again on main at {short}: {a.run_url}")
        return "closed"
    return "nothing"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for name in ("workflow", "conclusion", "event", "branch", "default-branch", "sha", "run-url", "repo"):
        ap.add_argument("--" + name, required=True)
    a = ap.parse_args(argv)
    print(act(a))
    return 0


if __name__ == "__main__":
    sys.exit(main())
