"""Main-break rerun and auto-revert (CUA-4001): watch list, culprit rule and loop guards."""
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / '.github/workflows'
sys.path.insert(0, str(ROOT / '.github/scripts'))
import main_break as mb  # noqa: E402

WF = yaml.safe_load((WORKFLOWS / 'ci-main-break.yml').read_text())

# Lanes that run on pull requests and on main but are not watched, and why.
EXCLUDED = {
    'Release: Auto-merge release PRs': 'release automation, not a check',
    'CD: Docs MCP Server': 'a deploy',
    'CI: Cua Driver Bench': 'a benchmark',
    'CI: Bench results and compare links': 'a report',
    'CI: Cua Driver desktop readiness (non-certifying)': 'non-certifying',
    'CI: Cua Spaces': 'fixed forward by policy (ci-postmerge-issue-bundles.yml)',
    'CI: Cua Spaces for macOS (SwiftUI)': 'fixed forward by policy (ci-postmerge-issue-bundles.yml)',
    'CI: Check eMudhra Distrust': 'depends on an outside certificate store',
    'CI: pi-durable crash campaign': 'a randomized campaign',
    'CI: cua-compat Daytona SDK conformance': 'depends on an outside service',
    'CI: cua-compat E2B SDK surface and coverage': 'depends on an outside service',
}


def runs_on_pr_and_main(path: Path) -> bool:
    on = yaml.safe_load(path.read_text()).get(True) or {}
    if not isinstance(on, dict) or 'pull_request' not in on or 'push' not in on:
        return False
    push = on['push'] or {}
    branches = push.get('branches') if isinstance(push, dict) else None
    if branches:
        return 'main' in branches
    return not (isinstance(push, dict) and push.get('tags'))


def test_watch_list_covers_every_gating_lane_on_main():
    listed = WF[True]['workflow_run']['workflows']
    watch = [w.strip() for w in WF['jobs']['main-break']['env']['WATCH'].splitlines() if w.strip()]
    assert listed == watch, 'the workflow_run trigger and WATCH must list the same lanes'
    expected = {yaml.safe_load(p.read_text())['name'] for p in WORKFLOWS.glob('*.yml') if runs_on_pr_and_main(p)}
    assert set(EXCLUDED) <= expected | set(EXCLUDED)
    assert expected - set(EXCLUDED) - set(listed) == set(), 'add these lanes to ci-main-break.yml (or to EXCLUDED, with a reason)'
    assert set(listed) - expected == set(), 'these watched names match no workflow that runs on pull requests and main'


def test_main_break_never_runs_pull_request_code():
    assert set(WF[True]) == {'workflow_run', 'schedule', 'workflow_dispatch'}
    checkout = [s for s in WF['jobs']['main-break']['steps'] if str(s.get('uses', '')).startswith('actions/checkout')]
    assert checkout and 'default_branch' in checkout[0]['with']['ref']


def test_is_revert():
    assert mb.is_revert('revert: fix(x): y (main broken: CI: Rust Linux unit)', [])
    assert mb.is_revert('Revert "feat: thing"', [])
    assert mb.is_revert('revert(spaces): thing', [])
    assert mb.is_revert('fix: thing', ['auto-revert'])
    assert not mb.is_revert('fix: handle reverted sessions', [])


def test_revert_title_passes_the_pr_title_rule():
    import re
    title = mb.revert_title('fix(spaces): keep the window', 'CI: Rust Linux unit')
    assert title == 'revert: fix(spaces): keep the window (main broken: CI: Rust Linux unit)'
    assert re.match(r'^(feat|fix|perf|docs|chore|ci|test|refactor|build|style|revert)(\([^)]+\))?!?: .+', title)


def test_previous_run_skips_cancelled_and_same_commit():
    failing = {'id': 50, 'head_sha': 'c'}
    runs = [
        {'id': 51, 'head_sha': 'd', 'status': 'completed', 'conclusion': 'success'},
        {'id': 49, 'head_sha': 'c', 'status': 'completed', 'conclusion': 'failure'},
        {'id': 48, 'head_sha': 'b', 'status': 'completed', 'conclusion': 'cancelled'},
        {'id': 47, 'head_sha': 'a', 'status': 'completed', 'conclusion': 'success'},
    ]
    assert mb.previous_run(runs, failing)['id'] == 47
    assert mb.previous_run(runs[:2], failing) is None


def test_pick_culprit():
    assert mb.pick_culprit(['c'], {}) == ('c', '')
    assert mb.pick_culprit(['a', 'b', 'c'], {'a': False, 'b': True, 'c': False}) == ('b', '')
    culprit, why = mb.pick_culprit(['a', 'b'], {'a': True, 'b': True})
    assert culprit is None and 'not reverting on a guess' in why
    culprit, why = mb.pick_culprit(['a', 'b'], {'a': False, 'b': False})
    assert culprit is None and 'reaches' in why
    assert mb.pick_culprit([], {})[0] is None


class FakeRepo(mb.Repo):
    def __init__(self, runs, compare, pull, recent=None, issues=None, branch_exists=False):
        super().__init__('trycua/cua', 'main', 'app-token', dry_run=True)
        self._runs, self._compare, self._pull, self._recent = runs, compare, pull, recent
        self._issues, self._branch = issues or [], branch_exists
        self.reverted = []

    def push_runs(self, workflow_id):
        return self._runs

    def commits_between(self, base, head):
        return self._compare

    def reaches(self, sha, workflow_file):
        return True

    def pull_for(self, sha):
        return self._pull

    def recent_auto_revert(self):
        return self._recent

    def branch_exists(self, name):
        return self._branch

    def already_reported(self, issue, marker):
        return False

    def open_revert(self, sha, pr, workflow, run, issue_hint):
        if self._recent:
            return None, 'cooldown'
        self.reverted.append(sha)
        return 'https://example.invalid/pr/1', ''


def run(attempt=2, conclusion='failure', rid=50, sha='c'):
    return {'id': rid, 'name': 'CI: Rust Linux unit', 'event': 'push', 'head_branch': 'main', 'workflow_id': 1,
            'head_sha': sha, 'run_attempt': attempt, 'conclusion': conclusion, 'status': 'completed',
            'html_url': 'https://example.invalid/run', 'path': '.github/workflows/ci-rust-linux.yml'}


GREEN_BEFORE = [{'id': 40, 'head_sha': 'b', 'status': 'completed', 'conclusion': 'success'}]
PR = {'number': 7, 'title': 'fix(x): y', 'labels': []}
WATCH = {'CI: Rust Linux unit'}


def test_first_failure_only_reruns():
    repo = FakeRepo(GREEN_BEFORE, ['c'], PR)
    mb.handle(repo, run(attempt=1), WATCH)
    assert repo.reverted == []


def test_second_failure_with_one_culprit_reverts():
    repo = FakeRepo(GREEN_BEFORE, ['c'], PR)
    mb.handle(repo, run(), WATCH)
    assert repo.reverted == ['c']


def test_never_reverts_a_revert():
    repo = FakeRepo(GREEN_BEFORE, ['c'], {'number': 8, 'title': 'revert: fix(x): y (main broken: z)', 'labels': []})
    mb.handle(repo, run(), WATCH)
    assert repo.reverted == []


def test_main_already_red_is_not_reverted():
    red = [{'id': 40, 'head_sha': 'b', 'status': 'completed', 'conclusion': 'failure', 'html_url': 'https://example.invalid/r'}]
    repo = FakeRepo(red, ['c'], PR)
    mb.handle(repo, run(), WATCH)
    assert repo.reverted == []


def test_green_again_is_left_alone():
    newer = GREEN_BEFORE + [{'id': 60, 'head_sha': 'd', 'status': 'completed', 'conclusion': 'success'}]
    repo = FakeRepo(newer, ['c'], PR)
    mb.handle(repo, run(), WATCH)
    assert repo.reverted == []


def test_unwatched_and_non_push_runs_are_ignored():
    repo = FakeRepo(GREEN_BEFORE, ['c'], PR)
    mb.handle(repo, dict(run(), event='pull_request'), WATCH)
    mb.handle(repo, dict(run(), name='Other'), WATCH)
    assert repo.reverted == []
