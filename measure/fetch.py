#!/usr/bin/env python3
"""Collect the raw GitHub Actions data that measure.py turns into the before/after table.

Writes JSON under measure/data/ so the analysis is reproducible offline:

  runs.json   - workflow runs (paged)
  jobs.json   - job records for the runs we care about
  logs/*.log  - job logs (for the sccache --show-stats block)

Usage:
  python3 measure/fetch.py            # uses `gh` for auth
"""
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DATA = ROOT / 'data'
LOGS = DATA / 'logs'
REPO = 'trycua/cua'
GH_ENV = {**os.environ,
          'GH_CONFIG_DIR': os.environ.get('GH_CONFIG_DIR', '/home/box/.config/gh'),
          'HOME': os.environ.get('HOME', '/home/box')}


def gh(*args, paginate=False):
    cmd = ['gh', 'api']
    if paginate:
        cmd.append('--paginate')
    cmd += list(args)
    out = subprocess.run(cmd, capture_output=True, text=True, env=GH_ENV)
    if out.returncode != 0:
        sys.exit(f"gh {' '.join(args)} failed: {out.stderr.strip()[:400]}")
    return out.stdout


def gh_json(*args, paginate=False):
    return json.loads(gh(*args, paginate=paginate))


def fetch_runs(pages=10):
    runs = []
    for page in range(1, pages + 1):
        try:
            d = gh_json(f'repos/{REPO}/actions/runs?per_page=100&page={page}')
        except SystemExit:
            break
        batch = d.get('workflow_runs', [])
        runs.extend(batch)
        if len(batch) < 100:
            break
    return runs


def fetch_workflows():
    wfs = []
    for page in range(1, 4):
        try:
            d = gh_json(f'repos/{REPO}/actions/workflows?per_page=100&page={page}')
        except SystemExit:
            break
        batch = d.get('workflows', [])
        wfs.extend(batch)
        if len(batch) < 100:
            break
    return wfs


def fetch_jobs(run_id):
    return gh_json(f'repos/{REPO}/actions/runs/{run_id}/jobs?per_page=100&filter=latest')


def fetch_log(job_id):
    """Job logs need the raw URL; gh api follows the redirect for us."""
    LOGS.mkdir(parents=True, exist_ok=True)
    dest = LOGS / f'{job_id}.log'
    if dest.exists() and dest.stat().st_size > 0:
        return dest
    text = gh(f'repos/{REPO}/actions/jobs/{job_id}/logs')
    # gh streams the log body straight through for this endpoint.
    dest.write_text(text, encoding='utf-8', errors='replace')
    return dest


def main():
    DATA.mkdir(parents=True, exist_ok=True)
    print('fetching workflow runs...', flush=True)
    runs = fetch_runs()
    (DATA / 'runs.json').write_text(json.dumps(runs))
    print(f'  {len(runs)} runs', flush=True)

    print('fetching workflows...', flush=True)
    wfs = fetch_workflows()
    (DATA / 'workflows.json').write_text(json.dumps(wfs))
    print(f'  {len(wfs)} workflows', flush=True)

    by_path = {w['path']: w for w in wfs}
    wanted = ['.github/workflows/ci-rust-linux.yml',
              '.github/workflows/ci-cua-spacesd.yml',
              '.github/workflows/e2e-cua-driver-release-gate.yml',
              '.github/workflows/ci-gate.yml']
    runs_by_wf = {}
    for path in wanted:
        w = by_path.get(path)
        if not w:
            print(f'  ! workflow not found: {path}', flush=True)
            continue
        d = gh_json(f"repos/{REPO}/actions/workflows/{w['id']}/runs?per_page=40")
        runs_by_wf[path] = d.get('workflow_runs', [])
        print(f'  {path}: {len(runs_by_wf[path])} runs', flush=True)
    (DATA / 'workflow_runs.json').write_text(json.dumps(runs_by_wf))

    print('fetching jobs...', flush=True)
    jobs = {}
    for path, rs in runs_by_wf.items():
        for r in rs[:25]:
            if r.get('conclusion') is None:
                continue
            jobs[str(r['id'])] = fetch_jobs(r['id']).get('jobs', [])
    (DATA / 'jobs.json').write_text(json.dumps(jobs))
    print(f'  {len(jobs)} run job-sets', flush=True)
    print('done. now run: python3 measure/measure.py', flush=True)


if __name__ == '__main__':
    main()
