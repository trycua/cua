"""ci-gate rechecks itself when a result it failed on turns green (CUA-3856)."""
import re
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / '.github/workflows'
sys.path.insert(0, str(ROOT / '.github/scripts'))
import ci_gate_recheck as recheck  # noqa: E402

RECHECK = yaml.safe_load((WORKFLOWS / 'ci-gate-recheck.yml').read_text())
GATE_TEXT = (WORKFLOWS / 'ci-gate.yml').read_text()


def gate_waits_on(path: Path) -> bool:
    """Mirror the gate's run filter: pull_request runs, minus the workflows it skips."""
    skip = re.search(r'test\(\\"(claude-auto-fix[^"]*)\\"\)', GATE_TEXT).group(1).replace('\\\\', '\\')
    if path.name in ('ci-gate.yml', 'ci-gate-recheck.yml') or re.search(skip, '/' + path.name):
        return False
    on = yaml.safe_load(path.read_text()).get(True) or {}
    keys = [on] if isinstance(on, str) else list(on)
    return 'pull_request' in keys


def test_recheck_listens_to_every_workflow_the_gate_waits_on():
    listed = set(RECHECK[True]['workflow_run']['workflows'])
    expected = {yaml.safe_load(p.read_text())['name'] for p in WORKFLOWS.glob('*.yml') if gate_waits_on(p)}
    assert expected - listed == set(), 'add these names to ci-gate-recheck.yml'
    assert listed - expected == set(), 'remove these names from ci-gate-recheck.yml'


def test_recheck_also_listens_to_the_sign_off_and_never_checks_out_pull_request_code():
    assert 'status' in RECHECK[True]
    steps = RECHECK['jobs']['recheck']['steps']
    checkout = [s for s in steps if str(s.get('uses', '')).startswith('actions/checkout')]
    assert checkout and 'ref' not in checkout[0].get('with', {})
    assert RECHECK['permissions'] == {'actions': 'write', 'contents': 'read', 'pull-requests': 'read'}


class FakeGitHub:
    def __init__(self, gate=None, pulls=None):
        self.gate, self.pulls, self.calls = gate, pulls or [], []

    def __call__(self, path, method='GET'):
        self.calls.append((method, path))
        if '/workflows/ci-gate.yml/runs' in path:
            return {'workflow_runs': [self.gate] if self.gate else []}
        if '/pulls?' in path:
            return self.pulls
        raise AssertionError(path)


GATE = {'id': 9, 'event': 'pull_request', 'status': 'completed', 'conclusion': 'failure',
        'created_at': '2026-11-08T10:00:00Z', 'updated_at': '2026-11-08T11:00:00Z',
        'head_sha': 'abc123', 'head_branch': 'fix', 'head_repository': {'full_name': 'o/r'}}
PR = {'number': 7, 'head': {'sha': 'abc123'}, 'labels': []}
LATER = recheck.ts('2026-11-08T12:00:00Z')
EARLIER = recheck.ts('2026-11-08T10:30:00Z')


@pytest.mark.parametrize(('gate', 'pulls', 'ok', 'at', 'rerun'), [
    (GATE, [PR], True, LATER, 9),                                           # rerun went green after the gate failed
    (GATE, [PR], False, LATER, None),                                       # still red: leave the gate alone
    (GATE, [PR], True, EARLIER, None),                                      # the gate already saw this result
    ({**GATE, 'status': 'in_progress'}, [PR], True, LATER, None),           # the running gate will see it
    ({**GATE, 'conclusion': 'success'}, [PR], True, LATER, None),
    ({**GATE, 'conclusion': 'cancelled'}, [PR], True, LATER, None),         # superseded by a newer push
    ({**GATE, 'conclusion': 'timed_out'}, [PR], True, LATER, 9),
    (GATE, [{**PR, 'head': {'sha': 'newer'}}], True, LATER, None),          # the pull request moved on
    (GATE, [], True, LATER, None),                                          # closed or merged
    (GATE, [{**PR, 'labels': [{'name': 'release-hold'}]}], True, LATER, None),
    (None, [PR], True, LATER, None),
])
def test_decide(monkeypatch, gate, pulls, ok, at, rerun):
    fake = FakeGitHub(gate, pulls)
    monkeypatch.setattr(recheck, 'gh', fake)
    reason, gate_id = recheck.decide('o/r', 'abc123', ok, at, 'CI: cua SDK')
    assert gate_id == rerun, reason
    assert all(method == 'GET' for method, _ in fake.calls)


def test_a_fork_pull_request_is_found_by_its_owner_and_branch(monkeypatch):
    fake = FakeGitHub({**GATE, 'head_repository': {'full_name': 'someone/cua'}}, [PR])
    monkeypatch.setattr(recheck, 'gh', fake)
    recheck.decide('o/r', 'abc123', True, LATER, 'x')
    assert ('GET', 'repos/o/r/pulls?state=open&head=someone:fix&per_page=10') in fake.calls
