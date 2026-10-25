#!/usr/bin/env python3
"""Post the state of watched GitHub workflows to Alertmanager (CUA-2227).

A copy of trycua/cloud scripts/ci/workflow_alerts.py; keep the two in step.

GitHub emails a failed scheduled workflow only to whoever last edited its cron
line, so a red nightly can go unseen for weeks. This turns each watched
workflow into one Alertmanager alert instead. Alertmanager routes
source="github-actions" to #alerts-staging. It never pages.

    workflow_alerts.py --watch "spaces-staging-smoke=staging" --event "$GITHUB_EVENT_PATH"
    workflow_alerts.py --watch "spaces-staging-smoke=staging" --sweep

--event reads a workflow_run event and reports that run. --sweep looks at the
latest finished run of every watched workflow and reports it, which refreshes a
red alert and posts the green one. Both build the same alert, so Alertmanager
treats them as one.

A failed run (failure, timed_out, startup_failure) is a firing alert that ends
after --ttl-hours unless refreshed. A successful run is a resolved alert, and
the Slack message for it is the "green again" note. Cancelled, skipped and other
conclusions change nothing. Standard library only.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

ALERTMANAGER_URL = "https://am.cua.ai/api/v2/alerts"
API = "https://api.github.com"
ALERTNAME = "GitHubWorkflowFailed"
FIRING = {"failure", "timed_out", "startup_failure"}
RESOLVED = {"success"}
# Triggers nobody is watching live. A pull_request run (a fork's too) never alerts.
EVENTS = {"schedule", "push", "repository_dispatch", "workflow_dispatch"}
# A release run is a push of its tag, and the gate is re-run by dispatching the workflow on it (cua-spaces-v0.15.0).
RELEASE_REF = re.compile(r"[A-Za-z0-9_.]+(?:-[A-Za-z0-9_.]+)*-v\d+\.\d+[A-Za-z0-9._+-]*")
WHY = {
    "failure": "a job failed",
    "timed_out": "the run timed out",
    "startup_failure": "GitHub rejected the run before any job started (a workflow file or permissions error; the run has no log)",
}


def parse_watch(entries: list[str]) -> dict[str, str]:
    """["name=environment", "name"] -> {name: environment}; environment defaults to ci."""
    watch: dict[str, str] = {}
    for entry in entries:
        name, _, environment = entry.partition("=")
        if name.strip():
            watch[name.strip()] = environment.strip() or "ci"
    return watch


def state_of(conclusion: str | None) -> str | None:
    if conclusion in FIRING:
        return "firing"
    if conclusion in RESOLVED:
        return "resolved"
    return None


def eligible(run: dict, repo: str, default_branch: str) -> bool:
    """A run that says something about the workflow's real state: of this repository, and either on the default
    branch or a push or dispatch on a release tag (cua-spaces-v0.15.0). A push to a feature branch, even one that
    breaks the workflow file, is not the pipeline's state."""
    head_repo = (run.get("head_repository") or {}).get("full_name")
    if head_repo and head_repo != repo:
        return False
    event = run.get("event")
    if event not in EVENTS:
        return False
    branch = run.get("head_branch") or ""
    if branch == default_branch:
        return True
    return event in ("push", "workflow_dispatch") and RELEASE_REF.fullmatch(branch) is not None


def iso(moment: dt.datetime) -> str:
    return moment.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def build_alert(run: dict, name: str, repo: str, environment: str, now: dt.datetime, ttl_hours: float, runbook: str = "") -> dict | None:
    """`name` is the workflow's name. A run's own "name" is its run-name when the workflow sets one."""
    state = state_of(run.get("conclusion"))
    if state is None:
        return None
    conclusion = run["conclusion"]
    sha = (run.get("head_sha") or "")[:7]
    where = "%s run %s (%s, %s)" % (name, run.get("run_number", "?"), run.get("event"), sha or "unknown commit")
    labels = {
        "alertname": ALERTNAME,
        "source": "github-actions",
        "severity": "warning",
        "environment": environment,
        "repo": repo,
        "workflow": name,
    }
    annotations = {"run_url": run.get("html_url", "")}
    if runbook:
        annotations["runbook"] = runbook
    if state == "firing":
        annotations["summary"] = "%s is red in %s" % (name, repo)
        annotations["description"] = "%s: %s." % (where, WHY[conclusion])
        starts, ends = run.get("run_started_at") or run.get("created_at"), iso(now + dt.timedelta(hours=ttl_hours))
    else:
        annotations["summary"] = "%s is green again in %s" % (name, repo)
        annotations["description"] = "%s succeeded." % where
        starts, ends = run.get("run_started_at") or run.get("created_at"), iso(now)
    return {"labels": labels, "annotations": annotations, "startsAt": starts, "endsAt": ends}


def http_json(url: str, token: str = "", data: object = None, attempts: int = 3):
    body = None if data is None else json.dumps(data).encode()
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "workflow-alerts"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    if token:
        headers["Authorization"] = "Bearer " + token
    for attempt in range(1, attempts + 1):
        request = urllib.request.Request(url, data=body, headers=headers, method="POST" if body is not None else "GET")
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                text = response.read().decode()
                return json.loads(text) if text.strip() else None
        except urllib.error.HTTPError as err:
            if err.code < 500 and err.code != 429 or attempt == attempts:
                raise
        except urllib.error.URLError:
            if attempt == attempts:
                raise
        time.sleep(2 * attempt)


def workflow_ids(repo: str, watch: dict[str, str], token: str) -> dict[str, int]:
    ids: dict[str, int] = {}
    page = 1
    while len(ids) < len(watch):
        listing = http_json("%s/repos/%s/actions/workflows?per_page=100&page=%d" % (API, repo, page), token)
        for workflow in listing.get("workflows", []):
            if workflow["name"] in watch:
                ids[workflow["name"]] = workflow["id"]
        if len(listing.get("workflows", [])) < 100:
            break
        page += 1
    for missing in sorted(set(watch) - set(ids)):
        print("::warning::watched workflow %r not found in %s" % (missing, repo))
    return ids


def latest_run(repo: str, workflow_id: int, token: str, default_branch: str, max_age_days: float | None, now: dt.datetime) -> dict | None:
    """The newest finished run of a workflow that can change its alert (eligible, failed or succeeded).
    Pages through newer pull request and cancelled runs; None when there is none within max_age_days."""
    for page in range(1, 6):
        runs = http_json("%s/repos/%s/actions/workflows/%d/runs?status=completed&per_page=100&page=%d" % (API, repo, workflow_id, page), token)
        batch = runs.get("workflow_runs", [])
        for run in batch:
            created = dt.datetime.fromisoformat(run["created_at"].replace("Z", "+00:00"))
            if max_age_days is not None and (now - created).total_seconds() > max_age_days * 86400:
                return None  # newest first: everything after this is older still
            if state_of(run.get("conclusion")) is not None and eligible(run, repo, default_branch):
                return run
        if len(batch) < 100:
            return None
    return None


def latest_runs(repo: str, watch: dict[str, str], token: str, default_branch: str, max_age_days: float, now: dt.datetime) -> list[tuple[str, dict]]:
    """(workflow name, newest finished eligible run) for each watched workflow, newer than max_age_days."""
    found = []
    for name, workflow_id in workflow_ids(repo, watch, token).items():
        run = latest_run(repo, workflow_id, token, default_branch, max_age_days, now)
        if run:
            found.append((name, run))
    return found


def post(alerts: list[dict], url: str) -> None:
    if alerts:
        http_json(url, data=alerts)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--watch", action="append", default=[], metavar="NAME[=ENVIRONMENT]", help="workflow name; repeat")
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--event", metavar="PATH", help="a workflow_run event payload ($GITHUB_EVENT_PATH)")
    mode.add_argument("--sweep", action="store_true", help="report the latest finished run of every watched workflow")
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    ap.add_argument("--default-branch", default=os.environ.get("DEFAULT_BRANCH", "main"))
    ap.add_argument("--ttl-hours", type=float, default=2.0, help="a red alert ends this long after the last report")
    ap.add_argument("--max-age-days", type=float, default=14.0, help="--sweep ignores runs older than this")
    ap.add_argument("--runbook", default="")
    ap.add_argument("--url", default=os.environ.get("ALERTMANAGER_URL", ALERTMANAGER_URL))
    ap.add_argument("--dry-run", action="store_true", help="print the alerts instead of posting them")
    args = ap.parse_args(argv)
    watch = parse_watch(args.watch)
    if not watch or not args.repo:
        ap.error("--watch and --repo (or GITHUB_REPOSITORY) are required")
    now = dt.datetime.now(dt.timezone.utc)

    if args.event:
        with open(args.event) as handle:
            event = json.load(handle)
        run = event.get("workflow_run") or {}
        repo = (event.get("repository") or {}).get("full_name") or args.repo
        default_branch = (event.get("repository") or {}).get("default_branch") or args.default_branch
        workflow = (event.get("workflow") or {}).get("name") or run.get("name")
        runs = [(workflow, run)] if workflow in watch and eligible(run, repo, default_branch) else []
        if runs and run.get("workflow_id"):
            # An older run can finish after a newer one (a re-run, a long build). Only the newest finished run may
            # change the alert, or a late success would clear a newer failure until the next sweep.
            token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""
            newest = latest_run(repo, run["workflow_id"], token, default_branch, None, now)
            if newest and newest["id"] != run.get("id"):
                print("run %s of %s is not the newest finished run (%s); the sweep reports that one" % (run.get("id"), workflow, newest["id"]))
                runs = []
    else:
        token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""
        runs = latest_runs(args.repo, watch, token, args.default_branch, args.max_age_days, now)
        repo = args.repo

    alerts = [a for a in (build_alert(r, n, repo, watch[n], now, args.ttl_hours, args.runbook) for n, r in runs) if a]
    for alert in alerts:
        state = "firing" if alert["endsAt"] > iso(now) else "resolved"
        print("%s %s: %s" % (state, alert["labels"]["workflow"], alert["annotations"]["description"]))
    if not alerts:
        print("nothing to report")
    if args.dry_run:
        print(json.dumps(alerts, indent=2))
        return 0
    post(alerts, args.url)
    return 0


if __name__ == "__main__":
    sys.exit(main())
