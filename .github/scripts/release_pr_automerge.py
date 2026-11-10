#!/usr/bin/env python3
"""Auto-merge release-please pull requests, and alert when one sits green (CUA-3559).

Every release-please PR (branch release-please--branches--*, opened by the release
App from this repository) gets `gh pr merge --auto --squash`. It then merges only
when the required checks (ci-gate and the rest of the ruleset) are green. The
`release-hold` label (or the older `hold`) is the only hold it respects. Switching off
auto-merge by hand is NOT a hold: the next run (on every PR update, and every 30 minutes)
turns it back on, which is how #6749 (cua-spaces 0.22.0) merged while held. The label stops it: a held PR has auto-merge switched off, and ci-gate fails while the
label is on (ci-gate.yml), so a hold also wins a race with this script.

    release_pr_automerge.py --repo trycua/cua                # all open release PRs
    release_pr_automerge.py --repo trycua/cua --pr 123       # one PR (pull_request)
    release_pr_automerge.py --repo trycua/cua --alert        # also alert on stale green PRs
    release_pr_automerge.py --repo trycua/cua --guard .github/scripts/release_please_tag_guard.cjs

--guard checks each PR body with the pinned release-please engine before auto-merge is
switched on (NODE_PATH must hold release-please@17.3.0). Release Please tags from the
merged body; text above the version heading makes it tag every component (#6350, 15 stray
v0.20.0 releases). A body that fails is repaired when the fix is only moving the heading
back to the top; otherwise auto-merge is switched off and the run fails.

Auto-merge is also held back while main has commits touching the component that the
release branch lacks (staleness above), and the workflow sweeps on every main push.

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
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import workflow_alerts  # noqa: E402

BRANCH_PREFIX = "release-please--branches--"
# `release-hold` is the documented label; `hold` is the older name and still works.
# Keep this list in step with the label check in ci-gate.yml.
HOLD_LABELS = ("release-hold", "hold")
GATE_CHECK = "ci-gate"
ALERTNAME = "ReleasePRStuck"


class BadReleaseBody(Exception):
    pass


def is_release_pr(pr: dict, repo: str) -> bool:
    owner = repo.split("/")[0]
    head_owner = (pr.get("headRepositoryOwner") or {}).get("login")
    return str(pr.get("headRefName", "")).startswith(BRANCH_PREFIX) and head_owner == owner and not pr.get("isDraft")


def on_hold(pr: dict) -> bool:
    return any(label.get("name") in HOLD_LABELS for label in pr.get("labels", []))


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


def body_guard(pr: dict, repo: str, guard: str) -> str | None:
    """None when the PR body tags only its own component; otherwise why not.

    Repairs the body in place when moving the version heading to the top is enough.
    """
    n = str(pr["number"])
    view = gh("pr", "view", n, "--repo", repo, "--json", "number,title,headRefName,body,mergeCommit")
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "pr.json")
        with open(src, "w") as fh:
            fh.write(view)
        if subprocess.run(["node", guard, "check", src], capture_output=True).returncode == 0:
            return None
        fixed = subprocess.run(["node", guard, "repair", src], capture_output=True, text=True)
        if fixed.returncode != 0:
            return "#%s: body would tag other components and cannot be repaired" % n
        out = os.path.join(tmp, "body.md")
        with open(out, "w") as fh:
            fh.write(fixed.stdout)
        gh("pr", "edit", n, "--repo", repo, "--body-file", out)
        print("#%s: moved the version heading back to the top of the body" % n)
    return None


FIELDS = "number,title,headRefName,headRepositoryOwner,isDraft,labels,autoMergeRequest,headRefOid"


def open_release_prs(repo: str, only: int | None) -> list[dict]:
    if only is not None:
        prs = [json.loads(gh("pr", "view", str(only), "--repo", repo, "--json", FIELDS))]
    else:
        prs = json.loads(gh("pr", "list", "--repo", repo, "--state", "open", "--base", "main", "--limit", "100", "--json", FIELDS))
    return [pr for pr in prs if is_release_pr(pr, repo)]


def reconcile(pr: dict, repo: str, guard: str | None = None, config: dict | None = None) -> str:
    n = str(pr["number"])
    if config is not None and not on_hold(pr):
        reason = staleness(pr, repo, config)
        if reason:
            if pr.get("autoMergeRequest"):
                gh("pr", "merge", n, "--repo", repo, "--disable-auto")
            return "#%s: stale (%s); auto-merge off until Release Please refreshes it" % (n, reason)
    if guard and not on_hold(pr):
        problem = body_guard(pr, repo, guard)
        if problem:
            if pr.get("autoMergeRequest"):
                gh("pr", "merge", n, "--repo", repo, "--disable-auto")
            raise BadReleaseBody(problem)
    if on_hold(pr):
        if pr.get("autoMergeRequest"):
            gh("pr", "merge", n, "--repo", repo, "--disable-auto")
            return "#%s: hold label, auto-merge disabled" % n
        return "#%s: hold label, left alone" % n
    if pr.get("autoMergeRequest"):
        return "#%s: auto-merge already on" % n
    gh("pr", "merge", n, "--repo", repo, "--auto", "--squash")
    return "#%s: auto-merge (squash) enabled" % n


def relevant_paths(config: dict, branch: str) -> tuple[list[str], list[str]]:
    """(component paths, paths Release Please ignores) for a release branch."""
    component = branch[len(BRANCH_PREFIX) :].split("components--", 1)[-1]
    paths: list[str] = []
    excluded: list[str] = []
    for path, package in config.get("packages", {}).items():
        if package.get("component") == component:
            paths.extend([path, *package.get("additional-paths", [])])
            excluded.extend(package.get("exclude-paths", []))
    return paths, excluded


def _under(file: str, prefixes: list[str]) -> bool:
    return any(file == p or file.startswith(p.rstrip("/") + "/") for p in prefixes)


def stale_files(files: list[str], paths: list[str], excluded: list[str]) -> list[str]:
    """Files that landed on main after the release branch was built and that change its release."""
    watched = [*paths, ".github/scripts", "release-please-config.json"]
    return [f for f in files if _under(f, watched) and not _under(f, excluded)]


def staleness(pr: dict, repo: str, config: dict) -> str | None:
    """Why the PR body may miss merged work, or None when it is current.

    Release Please runs once per main push, but GitHub keeps only one pending run, so
    a burst of pushes drops runs. A release PR merged before the surviving run finishes
    ships a changelog without the dropped PR, and the attribution preflight then fails
    the release (cua-driver-rs 0.41.5, #6834 merged 13 s after #6766).
    """
    paths, excluded = relevant_paths(config, pr["headRefName"])
    if not paths:
        return None
    compare = json.loads(gh("api", "repos/%s/compare/%s...main" % (repo, pr["headRefOid"])))
    files = [f["filename"] for f in compare.get("files", [])]
    if compare.get("ahead_by", 0) and len(files) >= 300:
        return "main moved by 300+ files since the branch was built"
    hit = stale_files(files, paths, excluded)
    if hit:
        return "main changed %d relevant file(s) after the branch was built, e.g. %s" % (len(hit), hit[0])
    return None


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
    ap.add_argument("--guard", help="release_please_tag_guard.cjs: check each body before enabling auto-merge")
    ap.add_argument("--config", default="release-please-config.json", help="skip auto-merge while main has newer relevant commits")
    ap.add_argument("--stale-hours", type=float, default=6.0)
    ap.add_argument("--dry-run", action="store_true", help="print the alerts instead of posting them")
    args = ap.parse_args(argv)
    now = dt.datetime.now(dt.timezone.utc)
    prs = open_release_prs(args.repo, args.pr)
    failed = False
    config = None
    if os.path.exists(args.config):
        with open(args.config, encoding="utf-8") as fh:
            config = json.load(fh)
    for pr in prs:
        try:
            print(reconcile(pr, args.repo, args.guard, config))
        except BadReleaseBody as bad:
            print("::error::%s. Keep the version heading (## [x.y.z](...)) first after '---'; put added notes below it." % bad)
            failed = True
    if not prs:
        print("no open release PRs")
    if args.alert:
        alerts = stale_alerts(prs, args.repo, args.stale_hours, now)
        for alert in alerts:
            print("ALERT " + alert["annotations"]["summary"])
        if not args.dry_run:
            workflow_alerts.post(alerts, workflow_alerts.ALERTMANAGER_URL)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
