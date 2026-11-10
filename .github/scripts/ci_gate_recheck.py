#!/usr/bin/env python3
"""Run `ci-gate` again when a result it failed on has since turned green (CUA-3856).

`ci-gate` is one long job that waits for every other workflow on the pull request head and
then gives one verdict. When a job fails, the gate fails. Rerunning that job alone used to leave
the gate red until someone also reran the gate. `.github/workflows/ci-gate-recheck.yml` calls
this script whenever a workflow the gate waits on completes (first run or rerun), and when the
`local-ci` sign-off status turns green. It reruns the newest gate run for that head commit when
all of these hold:

* the trigger is green (a workflow run that succeeded, or a successful `local-ci` status);
* the newest `ci-gate` run for the commit came from a pull request, is completed and failed;
* the gate finished before the green result arrived, so it has not seen it yet;
* the commit is still the head of an open pull request that has no hold label.

The rerun keeps the run, its event and its check name, so branch protection and auto-merge see
an ordinary `ci-gate` result. The gate recomputes everything from scratch, so a rerun can only
turn green when every workflow it waits for is green. Any doubt means "do nothing": the gate
stays as it was, never greener than its own evaluation.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime

GATE_WORKFLOW = 'ci-gate.yml'
HOLD_LABELS = {'release-hold', 'hold'}
GREEN = {'success', 'neutral', 'skipped'}


def gh(*args: str, method: str = 'GET') -> object:
    """`gh api` with a few retries; returns parsed JSON (None for an empty body)."""
    cmd = ['gh', 'api', '-X', method, *args]
    for attempt in range(4):
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode == 0:
            return json.loads(proc.stdout) if proc.stdout.strip() else None
        if attempt < 3:
            time.sleep(10 * (attempt + 1))
    raise RuntimeError(f'gh api {" ".join(args)} failed: {proc.stderr.strip()}')


def ts(value: str | None) -> float:
    return datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp() if value else 0.0


def newest_gate_run(repo: str, sha: str) -> dict | None:
    runs = gh(f'repos/{repo}/actions/workflows/{GATE_WORKFLOW}/runs?head_sha={sha}&per_page=50')
    runs = [r for r in (runs or {}).get('workflow_runs', []) if r.get('event') == 'pull_request']
    return max(runs, key=lambda r: (ts(r.get('created_at')), r.get('id', 0)), default=None)


def open_pull_request(repo: str, gate: dict) -> dict | None:
    """The open pull request whose head is the gate run's commit, or None."""
    head_repo = (gate.get('head_repository') or {}).get('full_name') or repo
    owner = head_repo.split('/')[0]
    branch = gate.get('head_branch') or ''
    if not branch:
        return None
    pulls = gh(f'repos/{repo}/pulls?state=open&head={owner}:{branch}&per_page=10') or []
    for pr in pulls:
        if (pr.get('head') or {}).get('sha') == gate.get('head_sha'):
            return pr
    return None


def decide(repo: str, sha: str, trigger_ok: bool, trigger_at: float, why: str) -> tuple[str, int | None]:
    """Return (reason, gate run id to rerun or None)."""
    if not trigger_ok:
        return f'{why} is not green; nothing to recheck', None
    gate = newest_gate_run(repo, sha)
    if gate is None:
        return f'no pull_request ci-gate run for {sha[:10]}', None
    if gate.get('status') != 'completed':
        return f'ci-gate run {gate["id"]} is still {gate.get("status")}; it will see {why} itself', None
    if gate.get('conclusion') not in ('failure', 'timed_out'):
        return f'ci-gate run {gate["id"]} concluded {gate.get("conclusion")}; nothing to recheck', None
    if trigger_at and ts(gate.get('updated_at')) >= trigger_at:
        return f'ci-gate run {gate["id"]} finished after {why}, so it already saw it', None
    pr = open_pull_request(repo, gate)
    if pr is None:
        return f'{sha[:10]} is no longer the head of an open pull request', None
    labels = {label.get('name') for label in pr.get('labels') or []}
    if labels & HOLD_LABELS:
        return f'#{pr["number"]} is on hold ({", ".join(sorted(labels & HOLD_LABELS))})', None
    return f'{why} is green after ci-gate run {gate["id"]} failed; rerunning it for #{pr["number"]}', gate['id']


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--event', required=True, help='path to the GitHub event payload')
    ap.add_argument('--event-name', required=True, choices=('workflow_run', 'status'))
    ap.add_argument('--dry-run', action='store_true')
    args = ap.parse_args()
    repo = os.environ['GITHUB_REPOSITORY']
    event = json.loads(open(args.event).read())
    if args.event_name == 'workflow_run':
        run = event['workflow_run']
        if run.get('event') != 'pull_request':
            print(f'{run.get("name")} ran for {run.get("event")}; the gate only reads pull request runs')
            return 0
        sha = run['head_sha']
        ok = run.get('conclusion') in GREEN
        at = ts(run.get('updated_at'))
        why = f'{run.get("name")} (run {run.get("id")}, attempt {run.get("run_attempt")})'
    else:
        if event.get('context') != 'local-ci':
            print(f'status {event.get("context")!r} is not the sign-off; ignoring')
            return 0
        sha = event['sha']
        ok = event.get('state') == 'success'
        at = ts(event.get('updated_at') or event.get('created_at'))
        why = 'the local-ci sign-off'
    reason, gate_id = decide(repo, sha, ok, at, why)
    print(reason)
    if gate_id is not None and not args.dry_run:
        gh(f'repos/{repo}/actions/runs/{gate_id}/rerun', method='POST')
    summary = os.environ.get('GITHUB_STEP_SUMMARY')
    if summary:
        with open(summary, 'a') as fh:
            fh.write(f'### ci-gate recheck\n\n{reason}\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
