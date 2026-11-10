#!/usr/bin/env python3
"""When a CI lane fails on main: rerun once, then revert the pull request that broke it (CUA-4001).

Called by .github/workflows/ci-main-break.yml for every finished push run on main of the lanes it
watches, and every 30 minutes as a sweep (in case an event was missed).

  1. First failed attempt: rerun the failed jobs once, to rule out a flake. Nothing else.
  2. Failed again: find the commit that broke it. The previous push run of the same workflow on main
     must be green, and among the commits since then exactly one may reach this workflow, by the
     same area map ci-gate uses (.github/ci-areas.json via ci_gate_scope.py). Anything less certain
     (main already red, two candidate commits, no pull request for the commit) is reported, not reverted.
  3. Open a revert pull request, "revert: <original title> (main broken: <workflow>)", label
     `auto-revert`, and queue it with `gh pr merge --auto --squash`. It goes through ci-gate and the
     merge queue like any other pull request.
  4. Open (or update) one issue per workflow, label `main-broken`, linking the run, the culprit and the
     revert. A later green push run of that workflow closes it.

Loop guards: a revert is never reverted (title `revert:` / `Revert "`, or label `auto-revert`), at
most one auto-revert pull request is opened per hour, one per culprit, and nothing is reverted when
main has gone green again since. Standard library only; talks to GitHub through the gh CLI.

    main_break.py --repo trycua/cua --event "$GITHUB_EVENT_PATH" --watch "CI: Rust Linux unit" ...
    main_break.py --repo trycua/cua --sweep --watch ...
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

ISSUE_LABEL = 'main-broken'
REVERT_LABEL = 'auto-revert'
FAILED = {'failure', 'timed_out'}
REVERT_COOLDOWN = dt.timedelta(hours=1)
SWEEP_MAX_AGE = dt.timedelta(hours=6)
SWEEP_RERUN_GRACE = dt.timedelta(minutes=20)


def gh(*args: str, token: str | None = None, data: str | None = None, check: bool = True) -> str:
    env = dict(os.environ)
    if token:
        env['GH_TOKEN'] = token
    for attempt in range(3):
        proc = subprocess.run(['gh', *args], capture_output=True, text=True, env=env, input=data)
        if proc.returncode == 0 or not check:
            return proc.stdout
        if attempt < 2:
            time.sleep(10 * (attempt + 1))
    raise RuntimeError(f'gh {" ".join(args[:3])} failed: {proc.stderr.strip()[:500]}')


def api(path: str, *args: str, token: str | None = None) -> object:
    out = gh('api', path, *args, token=token)
    return json.loads(out) if out.strip() else None


def ts(value: str | None) -> dt.datetime:
    return dt.datetime.fromisoformat((value or '1970-01-01T00:00:00Z').replace('Z', '+00:00'))


def now() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc)


# ---------------------------------------------------------------- pure decisions (unit-tested)

def is_revert(title: str, labels: list[str]) -> bool:
    return (REVERT_LABEL in labels
            or bool(re.match(r'\s*(revert(\([^)]*\))?!?:|Revert ")', title or '', re.IGNORECASE)))


def revert_title(original: str, workflow: str) -> str:
    return f'revert: {original.strip()} (main broken: {workflow})'


def previous_run(runs: list[dict], failing: dict) -> dict | None:
    """The newest completed push run of the same workflow on an older commit than `failing`."""
    older = [r for r in runs if r['id'] < failing['id'] and r['head_sha'] != failing['head_sha']
             and r.get('status') == 'completed' and r.get('conclusion') not in ('cancelled', 'skipped')]
    return max(older, key=lambda r: r['id'], default=None)


def pick_culprit(commits: list[str], reaches: dict[str, bool]) -> tuple[str | None, str]:
    """`commits`: oldest to newest since the last green run. `reaches`: does a commit reach the workflow."""
    if not commits:
        return None, 'no commits between the last green run and this one'
    candidates = [c for c in commits if reaches.get(c, True)]
    if len(candidates) == 1:
        return candidates[0], ''
    if not candidates:
        return None, f'none of the {len(commits)} commits since the last green run reaches this workflow'
    return None, (f'{len(candidates)} commits since the last green run reach this workflow '
                  f'({", ".join(c[:10] for c in candidates[:8])}); not reverting on a guess')


# ---------------------------------------------------------------- GitHub reads

class Repo:
    def __init__(self, repo: str, default_branch: str, app_token: str | None, dry_run: bool):
        self.repo, self.branch, self.app_token, self.dry_run = repo, default_branch, app_token, dry_run

    def push_runs(self, workflow_id: int) -> list[dict]:
        data = api(f'repos/{self.repo}/actions/workflows/{workflow_id}/runs?branch={self.branch}&event=push&per_page=50')
        return (data or {}).get('workflow_runs', [])

    def commits_between(self, base: str, head: str) -> list[str] | None:
        data = api(f'repos/{self.repo}/compare/{base}...{head}?per_page=100') or {}
        if data.get('status') not in ('ahead',) or data.get('total_commits', 0) > 100:
            return None
        return [c['sha'] for c in data.get('commits', [])]

    def reaches(self, sha: str, workflow_file: str) -> bool:
        """Whether commit `sha` changes anything this workflow covers, by ci-gate's own area map."""
        try:
            import ci_gate_scope as scope_mod
            files = gh('api', '--paginate', f'repos/{self.repo}/commits/{sha}', '--jq', '.files[] | .filename, (.previous_filename // empty)')
            names = [f for f in files.splitlines() if f]
            if not names or len(names) >= 3000:
                return True
            _, ignored = scope_mod.scope(names, scope_mod.load_map(), scope_mod.load_graph())
            return Path(workflow_file).name not in ignored
        except Exception as err:  # any doubt counts as "reaches"; that only makes the culprit less certain
            print(f'::warning::cannot scope {sha[:10]}: {err}')
            return True

    def pull_for(self, sha: str) -> dict | None:
        pulls = api(f'repos/{self.repo}/commits/{sha}/pulls') or []
        merged = [p for p in pulls if p.get('merged_at') and (p.get('base') or {}).get('ref') == self.branch]
        return merged[0] if merged else None

    def recent_auto_revert(self) -> str | None:
        since = (now() - REVERT_COOLDOWN).strftime('%Y-%m-%dT%H:%M:%SZ')
        q = f'repo:{self.repo} is:pr label:{REVERT_LABEL} created:>={since}'
        data = api('search/issues', '-X', 'GET', '-f', f'q={q}', '-f', 'per_page=5') or {}
        items = data.get('items') or []
        return items[0]['html_url'] if items else None

    def branch_exists(self, name: str) -> bool:
        out = gh('api', f'repos/{self.repo}/branches/{name}', '--jq', '.name', check=False)
        return out.strip() == name

    # ------------------------------------------------------------ issue

    def ensure_label(self, name: str, color: str, desc: str) -> None:
        if not self.dry_run:
            gh('label', 'create', name, '--repo', self.repo, '--color', color, '--description', desc, check=False)

    _issues: list[dict] | None = None

    def open_issues(self) -> list[dict]:
        if self._issues is None:
            issues = api(f'repos/{self.repo}/issues?state=open&labels={ISSUE_LABEL}&per_page=100') or []
            self._issues = [i for i in issues if 'pull_request' not in i]
        return self._issues

    def open_issue(self, workflow: str) -> dict | None:
        title = issue_title(workflow)
        return next((i for i in self.open_issues() if i.get('title') == title), None)

    def already_reported(self, issue: dict | None, marker: str) -> bool:
        if not issue:
            return False
        if marker in (issue.get('body') or ''):
            return True
        comments = gh('api', '--paginate', f'repos/{self.repo}/issues/{issue["number"]}/comments', '--jq', '.[].body')
        return marker in comments

    def report(self, workflow: str, text: str, marker: str) -> str:
        issue = self.open_issue(workflow)
        body = f'{text}\n\n{marker}'
        if self.dry_run:
            print(f'[dry-run] issue for {workflow}:\n{body}')
            return ''
        if issue:
            gh('api', '-X', 'POST', f'repos/{self.repo}/issues/{issue["number"]}/comments', '-f', f'body={body}')
            return issue['html_url']
        self.ensure_label(ISSUE_LABEL, 'b60205', 'A CI lane is failing on main (opened by ci-main-break.yml)')
        out = api(f'repos/{self.repo}/issues', '-X', 'POST', '-f', f'title={issue_title(workflow)}',
                  '-f', f'body={body}', '-f', f'labels[]={ISSUE_LABEL}')
        self._issues = None
        return out['html_url']

    def close(self, workflow: str, run: dict) -> None:
        issue = self.open_issue(workflow)
        if not issue:
            return
        text = f'{workflow} is green again on main at {run["head_sha"][:10]} ([run]({run["html_url"]})). Closing.'
        print(text)
        if not self.dry_run:
            gh('api', '-X', 'POST', f'repos/{self.repo}/issues/{issue["number"]}/comments', '-f', f'body={text}')
            gh('api', '-X', 'PATCH', f'repos/{self.repo}/issues/{issue["number"]}', '-f', 'state=closed', '-f', 'state_reason=completed')

    # ------------------------------------------------------------ revert

    def open_revert(self, sha: str, pr: dict, workflow: str, run: dict, issue_hint: str) -> tuple[str | None, str]:
        if not self.app_token:
            return None, 'no App token, so no revert pull request (a pull request opened with the job token would not run CI)'
        branch = f'auto-revert/pr-{pr["number"]}'
        if self.branch_exists(branch):
            return None, f'a revert branch for #{pr["number"]} already exists ({branch})'
        recent = self.recent_auto_revert()
        if recent:
            return None, f'an auto-revert was already opened in the last hour ({recent}); one per hour at most'
        if self.dry_run:
            return f'[dry-run] would revert {sha[:10]} on {branch}', ''
        git = lambda *a: subprocess.run(['git', *a], capture_output=True, text=True)  # noqa: E731
        git('fetch', '--no-tags', 'origin', self.branch)
        git('checkout', '-B', branch, f'origin/{self.branch}')
        res = git('-c', 'user.name=cua-main-break', '-c', 'user.email=ci@trycua.com', 'revert', '--no-edit', sha)
        if res.returncode != 0:
            git('revert', '--abort')
            return None, f'`git revert {sha[:10]}` does not apply cleanly on main: {(res.stdout + res.stderr).strip()[:300]}'
        url = f'https://x-access-token:{self.app_token}@github.com/{self.repo}.git'
        push = git('push', url, f'HEAD:refs/heads/{branch}')
        if push.returncode != 0:
            return None, 'pushing the revert branch failed: ' + push.stderr.replace(self.app_token, '***')[:300]
        self.ensure_label(REVERT_LABEL, '5319e7', 'Opened by ci-main-break.yml to revert a change that broke main')
        title = revert_title(pr['title'], workflow)
        body = (f'Automatic revert of #{pr["number"]} ({sha[:10]}): **{workflow}** failed on main twice '
                f'(first run and one rerun): {run["html_url"]}\n\n'
                f'It was the only commit since the last green run of {workflow} that reaches it (ci-areas.json).\n'
                f'To reland: fix the failure on a branch that reverts this revert, and open a new pull request.\n\n'
                f'Tracking: {issue_hint or "the main-broken issue"}. Opened by `.github/workflows/ci-main-break.yml` (CUA-4001).')
        new = gh('pr', 'create', '--repo', self.repo, '--base', self.branch, '--head', branch, '--title', title,
                 '--body', body, '--label', REVERT_LABEL, token=self.app_token).strip()
        merged = gh('pr', 'merge', new, '--repo', self.repo, '--auto', '--squash', token=self.app_token, check=False)
        print(merged)
        return new, ''


def issue_title(workflow: str) -> str:
    return f'main broken: {workflow}'


# ---------------------------------------------------------------- the decision for one run

def handle(gh_repo: Repo, run: dict, watch: set[str]) -> None:
    name = run.get('name') or ''
    if name not in watch:
        print(f'{name!r} is not watched')
        return
    if run.get('event') != 'push' or run.get('head_branch') != gh_repo.branch:
        print(f'{name}: {run.get("event")} on {run.get("head_branch")}; only push runs on {gh_repo.branch} count')
        return
    if run.get('conclusion') not in FAILED:
        print(f'{name}: {run.get("conclusion")}; nothing to do')
        return
    runs = gh_repo.push_runs(run['workflow_id'])
    newer = [r for r in runs if r['id'] > run['id'] and r.get('status') == 'completed']
    if any(r.get('conclusion') == 'success' for r in newer):
        print(f'{name}: a newer run on main is already green; not acting on run {run["id"]}')
        return
    attempt = run.get('run_attempt') or 1
    if attempt < 2:
        print(f'{name}: failed on {run["head_sha"][:10]} (attempt 1); rerunning the failed jobs once to rule out a flake')
        if not gh_repo.dry_run:
            gh('api', '-X', 'POST', f'repos/{gh_repo.repo}/actions/runs/{run["id"]}/rerun-failed-jobs', check=False)
        return

    marker = f'<!-- main-break run={run["id"]} -->'
    if gh_repo.already_reported(gh_repo.open_issue(name), marker):
        print(f'{name}: run {run["id"]} was already handled')
        return

    sha = run['head_sha']
    lines = [f'**{name}** failed on main at {sha[:10]}, and again on the rerun: {run["html_url"]}']
    prev = previous_run(runs, run)
    culprit, why = None, ''
    if prev is None:
        why = 'no earlier run of this workflow on main to compare with'
    elif prev.get('conclusion') != 'success':
        why = f'main was already red for this workflow before {sha[:10]} ({prev["html_url"]}); the first break was reported then'
    else:
        commits = gh_repo.commits_between(prev['head_sha'], sha)
        if commits is None:
            why = f'cannot list the commits between {prev["head_sha"][:10]} and {sha[:10]}'
        else:
            reaches = {c: gh_repo.reaches(c, run.get('path') or '') for c in commits} if len(commits) > 1 else {}
            culprit, why = pick_culprit(commits, reaches)

    pr = gh_repo.pull_for(culprit) if culprit else None
    if culprit and not pr:
        why = f'{culprit[:10]} has no merged pull request'
    revert_url = None
    if pr:
        labels = [l.get('name') for l in pr.get('labels') or []]
        lines.append(f'Culprit: #{pr["number"]} {pr["title"]} ({culprit[:10]}).')
        if is_revert(pr['title'], labels):
            why = f'#{pr["number"]} is itself a revert; reverts are never reverted automatically'
        else:
            issue = gh_repo.open_issue(name)
            revert_url, why = gh_repo.open_revert(culprit, pr, name, run, issue['html_url'] if issue else '')
    if revert_url:
        lines.append(f'Revert pull request (auto-merge queued): {revert_url}')
    else:
        lines.append(f'No automatic revert: {why}. A person needs to fix or revert this.')
    url = gh_repo.report(name, '\n\n'.join(lines), marker)
    print('\n'.join(lines))
    if url:
        print(f'issue: {url}')
    summary = os.environ.get('GITHUB_STEP_SUMMARY')
    if summary:
        with open(summary, 'a') as fh:
            fh.write('### main break\n\n' + '\n\n'.join(lines) + '\n')


def sweep(gh_repo: Repo, watch: set[str]) -> None:
    """Twice an hour: close the issues of lanes that are green again, and catch failures the
    workflow_run events missed (the newest push run on main of each watched lane)."""
    workflows = (api(f'repos/{gh_repo.repo}/actions/workflows?per_page=100') or {}).get('workflows', [])
    for wf in workflows:
        if wf.get('name') not in watch:
            continue
        runs = [r for r in gh_repo.push_runs(wf['id']) if r.get('status') == 'completed']
        if not runs:
            continue
        run = max(runs, key=lambda r: r['id'])
        if run.get('conclusion') == 'success':
            if gh_repo.open_issue(run['name']):
                gh_repo.close(run['name'], run)
            continue
        age = now() - ts(run.get('updated_at'))
        if run.get('conclusion') not in FAILED or age > SWEEP_MAX_AGE:
            continue
        if (run.get('run_attempt') or 1) < 2 and age < SWEEP_RERUN_GRACE:
            continue  # the event path is handling its rerun
        handle(gh_repo, run, watch)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--repo', required=True)
    ap.add_argument('--default-branch', default='main')
    ap.add_argument('--watch', action='append', default=[])
    ap.add_argument('--event', help='workflow_run event payload')
    ap.add_argument('--sweep', action='store_true')
    ap.add_argument('--dry-run', action='store_true')
    args = ap.parse_args(argv)
    gh_repo = Repo(args.repo, args.default_branch, os.environ.get('APP_TOKEN') or None, args.dry_run)
    watch = {w.strip() for w in args.watch if w.strip()}
    if args.event:
        handle(gh_repo, json.loads(Path(args.event).read_text())['workflow_run'], watch)
    if args.sweep:
        sweep(gh_repo, watch)
    return 0


if __name__ == '__main__':
    sys.exit(main())
