#!/usr/bin/env python3
"""Re-run "cua-spacesd pin is published" on open pull requests where it failed.

A Release Please PR for Cua Spaces or the cua SDK is opened while the cua-spacesd release it pins
(libs/cua-spacesd/VERSION) is still a draft that cd-cua-spacesd.yml is building. The check
(ci-release-spacesd-pin.yml) runs once, fails, and stayed red after the release published until
someone reran it by hand (#8155, #8174, 18 Nov 2026).

`.github/workflows/ci-spacesd-pin-recheck.yml` calls this when a cua-spacesd release is published.
For every open pull request whose newest run of the pin check failed, it reruns the failed job.
The job re-reads the release itself, so it only turns green when the pinned release really is
published with its assets; a PR pinning some other version just fails again. Runs that are not the
newest for their commit, and pull requests that closed or moved on, are left alone.
"""
from __future__ import annotations

import argparse
import os
import sys

from ci_gate_recheck import gh, ts

PIN_WORKFLOW = 'ci-release-spacesd-pin.yml'
TAG_PREFIX = 'cua-spacesd-v'
FAILED = {'failure', 'timed_out'}


def newest_runs(repo: str) -> list[dict]:
    """Newest pull_request run of the pin check for each head commit."""
    data = gh(f'repos/{repo}/actions/workflows/{PIN_WORKFLOW}/runs?event=pull_request&per_page=100')
    newest: dict[str, dict] = {}
    for run in (data or {}).get('workflow_runs', []):
        key = run.get('head_sha')
        if key and (key not in newest or (ts(run.get('created_at')), run['id']) > (ts(newest[key].get('created_at')), newest[key]['id'])):
            newest[key] = run
    return list(newest.values())


def is_open_head(repo: str, run: dict) -> int | None:
    """The number of the open pull request whose head is this run's commit, else None."""
    owner = ((run.get('head_repository') or {}).get('full_name') or repo).split('/')[0]
    branch = run.get('head_branch') or ''
    if not branch:
        return None
    for pr in gh(f'repos/{repo}/pulls?state=open&head={owner}:{branch}&per_page=10') or []:
        if (pr.get('head') or {}).get('sha') == run.get('head_sha'):
            return pr['number']
    return None


def runs_to_rerun(repo: str) -> list[tuple[int, int]]:
    """(pull request number, run id) for each failed, still-current pin check."""
    out = []
    for run in newest_runs(repo):
        if run.get('status') != 'completed' or run.get('conclusion') not in FAILED:
            continue
        number = is_open_head(repo, run)
        if number is not None:
            out.append((number, run['id']))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--tag', required=True, help='the release tag that was just published')
    ap.add_argument('--dry-run', action='store_true')
    args = ap.parse_args()
    if not args.tag.startswith(TAG_PREFIX):
        print(f'{args.tag} is not a cua-spacesd release; nothing to recheck')
        return 0
    repo = os.environ['GITHUB_REPOSITORY']
    lines = []
    for number, run_id in runs_to_rerun(repo):
        lines.append(f'#{number}: rerunning failed job of run {run_id}')
        if not args.dry_run:
            gh(f'repos/{repo}/actions/runs/{run_id}/rerun-failed-jobs', method='POST')
    if not lines:
        lines.append('no open pull request has a failed pin check')
    print('\n'.join(lines))
    summary = os.environ.get('GITHUB_STEP_SUMMARY')
    if summary:
        with open(summary, 'a') as fh:
            fh.write(f'### cua-spacesd pin recheck ({args.tag})\n\n' + '\n'.join(f'- {x}' for x in lines) + '\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
