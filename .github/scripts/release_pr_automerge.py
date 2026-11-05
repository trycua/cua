#!/usr/bin/env python3
"""Auto-merge release-please pull requests, and alert when one sits green (CUA-3559).

Every release-please PR (branch release-please--branches--*, opened by the release
App from this repository) gets `gh pr merge --auto --squash`. It then merges only
when the required checks (ci-gate and the rest of the ruleset) are green. The `hold`
label stops it: a held PR has auto-merge switched off, and ci-gate fails while the
label is on (ci-gate.yml), so a hold also wins a race with this script.

    release_pr_automerge.py --repo trycua/cua                # all open release PRs
    release_pr_automerge.py --repo trycua/cua --pr 123       # one PR (pull_request_target)
    release_pr_automerge.py --repo trycua/cua --alert        # also alert on stale green PRs

--alert posts one ReleasePRStuck alert to Alertmanager (same path as
workflow_alerts.py) for each release PR whose ci-gate has been green for more than
--stale-hours (6) and that is still open and not on hold. The alert expires in
an hour unless the next sweep refreshes it, so it clears itself once the PR merges.
Needs the `gh` CLI with GH_TOKEN set. Standard library only.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import workflow_alerts  # noqa: E402

BRANCH_PREFIX = "release-please--branches--"
HOLD_LABEL = "hold"
GATE_CHECK = "ci-gate"
ALERTNAME = "ReleasePRStuck"


def is_release_pr(pr: dict, repo: str) -> bool:
    owner = repo.split("/")[0]
    head_owner = (pr.get("headRepositoryOwner") or {}).get("login")
    return str(pr.get("headRefName", "")).startswith(BRANCH_PREFIX) and head_owner == owner and not pr.get("isDraft")


def on_hold(pr: dict) -> bool:
    return any(label.get("name") == HOLD_LABEL for label in pr.get("labels", []))


def parse_time(text: str) -> dt.datetime:
    return dt.datetime.fromisoformat(text.replace("Z", "+00:00"))


def gate_green_since(check_runs: list[dict]) -> dt.datetime | None:
    """When the latest ci-gate run on the head commit went green, or None if it is not green."""
    gates = [run for run in check_runs if run.get("name") == GATE_CHECK and run.get("completed_at")]
    if not gates:
        return None
    latest = max(gates, key=lambda run: run["completed_at"])
    return parse_time(latest["completed_at"]) if latest.get("conclusion") == "success" else None


def build_alert(pr: dict, repo: str, green_since: dt.datetime, now: dt.datetime, stale_hours: float) -> dict:
    hours = (now - green_since).total_seconds() / 3600
    url = "https://github.com/%s/pull/%s" % (repo, pr["number"])
    return {
        "labels": {
            "alertname": ALERTNAME,
            "source": "github-actions",
            "severity": "warning",
            "environment": "release",
            "repo": repo,
            "pr": str(pr["number"]),
        },
        "annotations": {
            "summary": "Release PR #%s has been green for %.0f h without merging in %s" % (pr["number"], hours, repo),
            "description": "%s (%s) passed ci-gate more than %g h ago and is still open and not on hold. "
            "Check that auto-merge is on and which required check is missing." % (pr.get("title", ""), url, stale_hours),
            "pr_url": url,
        },
        "startsAt": workflow_alerts.iso(green_since + dt.timedelta(hours=stale_hours)),
        "endsAt": workflow_alerts.iso(now + dt.timedelta(hours=1)),
    }


def gh(*args: str) -> str:
    done = subprocess.run(["gh", *args], capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit("gh %s failed: %s" % (" ".join(args[:3]), done.stderr.strip()))
    return done.stdout


FIELDS = "number,title,headRefName,headRepositoryOwner,isDraft,labels,autoMergeRequest,headRefOid"


def open_release_prs(repo: str, only: int | None) -> list[dict]:
    if only is not None:
        prs = [json.loads(gh("pr", "view", str(only), "--repo", repo, "--json", FIELDS))]
    else:
        prs = json.loads(gh("pr", "list", "--repo", repo, "--state", "open", "--base", "main", "--limit", "100", "--json", FIELDS))
    return [pr for pr in prs if is_release_pr(pr, repo)]


def reconcile(pr: dict, repo: str) -> str:
    n = str(pr["number"])
    if on_hold(pr):
        if pr.get("autoMergeRequest"):
            gh("pr", "merge", n, "--repo", repo, "--disable-auto")
            return "#%s: hold label, auto-merge disabled" % n
        return "#%s: hold label, left alone" % n
    if pr.get("autoMergeRequest"):
        return "#%s: auto-merge already on" % n
    gh("pr", "merge", n, "--repo", repo, "--auto", "--squash")
    return "#%s: auto-merge (squash) enabled" % n


def stale_alerts(prs: list[dict], repo: str, stale_hours: float, now: dt.datetime) -> list[dict]:
    alerts = []
    for pr in prs:
        if on_hold(pr):
            continue
        path = "repos/%s/commits/%s/check-runs?check_name=%s&per_page=100" % (repo, pr["headRefOid"], GATE_CHECK)
        green = gate_green_since(json.loads(gh("api", path))["check_runs"])
        if green is not None and now - green > dt.timedelta(hours=stale_hours):
            alerts.append(build_alert(pr, repo, green, now, stale_hours))
    return alerts


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--repo", required=True)
    ap.add_argument("--pr", type=int)
    ap.add_argument("--alert", action="store_true", help="post ReleasePRStuck alerts for stale green release PRs")
    ap.add_argument("--stale-hours", type=float, default=6.0)
    ap.add_argument("--dry-run", action="store_true", help="print the alerts instead of posting them")
    args = ap.parse_args(argv)
    now = dt.datetime.now(dt.timezone.utc)
    prs = open_release_prs(args.repo, args.pr)
    for pr in prs:
        print(reconcile(pr, args.repo))
    if not prs:
        print("no open release PRs")
    if args.alert:
        alerts = stale_alerts(prs, args.repo, args.stale_hours, now)
        for alert in alerts:
            print("ALERT " + alert["annotations"]["summary"])
        if not args.dry_run:
            workflow_alerts.post(alerts, workflow_alerts.ALERTMANAGER_URL)
    return 0


if __name__ == "__main__":
    sys.exit(main())
