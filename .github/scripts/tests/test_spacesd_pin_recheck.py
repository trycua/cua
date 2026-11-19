"""The cua-spacesd pin check reruns itself when a cua-spacesd release publishes."""
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / '.github/scripts'))
import spacesd_pin_recheck as recheck  # noqa: E402

WORKFLOW = yaml.safe_load((ROOT / '.github/workflows/ci-spacesd-pin-recheck.yml').read_text())
PIN = yaml.safe_load((ROOT / '.github/workflows/ci-release-spacesd-pin.yml').read_text())


def test_workflow_triggers_and_permissions_are_minimal():
    on = WORKFLOW[True]
    assert on['release'] == {'types': ['published']}
    assert on['workflow_run']['workflows'] == [yaml.safe_load((ROOT / '.github/workflows/cd-cua-spacesd.yml').read_text())['name']]
    assert WORKFLOW['permissions'] == {'actions': 'write', 'contents': 'read'}
    assert 'cua-spacesd-v' in WORKFLOW['jobs']['recheck']['if']


def test_workflow_never_checks_out_pull_request_code_and_pins_actions():
    steps = WORKFLOW['jobs']['recheck']['steps']
    checkout = steps[0]
    assert checkout['uses'].split('@')[1].split()[0].isalnum() and len(checkout['uses'].split('@')[1].split()[0]) == 40
    assert 'ref' not in checkout['with']
    assert PIN['jobs']['spacesd-published']['name'] == 'cua-spacesd pin is published'
    assert 'ci-release-spacesd-pin.yml' == recheck.PIN_WORKFLOW


def run(id, sha, conclusion='failure', created='2026-11-18T14:27:00Z', status='completed', branch='release-please--branches--main'):
    return {'id': id, 'head_sha': sha, 'conclusion': conclusion, 'status': status, 'created_at': created,
            'head_branch': branch, 'head_repository': {'full_name': 'o/r'}}


class FakeGitHub:
    def __init__(self, runs, open_heads):
        self.runs, self.open_heads, self.calls = runs, open_heads, []

    def __call__(self, path, method='GET'):
        self.calls.append((method, path))
        if '/workflows/ci-release-spacesd-pin.yml/runs' in path:
            return {'workflow_runs': self.runs}
        if '/pulls?' in path:
            return [{'number': n, 'head': {'sha': s}} for n, s in self.open_heads]
        if path.endswith('/rerun-failed-jobs'):
            return None
        raise AssertionError(path)


@pytest.mark.parametrize(('runs', 'heads', 'expected'), [
    ([run(1, 'a')], [(8155, 'a')], [(8155, 1)]),
    ([run(1, 'a', 'success')], [(8155, 'a')], []),
    ([run(1, 'a'), run(2, 'a', 'success', '2026-11-18T15:00:00Z')], [(8155, 'a')], []),    # already green
    ([run(1, 'a', 'success'), run(2, 'a', created='2026-11-18T15:00:00Z')], [(8155, 'a')], [(8155, 2)]),
    ([run(1, 'a', 'cancelled')], [(8155, 'a')], []),
    ([run(1, 'a', None, status='in_progress')], [(8155, 'a')], []),
    ([run(1, 'a')], [], []),                                                               # closed or merged
    ([run(1, 'a')], [(8155, 'newer')], []),                                                # the PR moved on
    ([run(1, 'a', 'timed_out')], [(8155, 'a')], [(8155, 1)]),
])
def test_runs_to_rerun(monkeypatch, runs, heads, expected):
    monkeypatch.setattr(recheck, 'gh', FakeGitHub(runs, heads))
    assert recheck.runs_to_rerun('o/r') == expected


def test_main_reruns_only_failed_jobs_and_ignores_other_tags(monkeypatch, capsys):
    fake = FakeGitHub([run(1, 'a')], [(8174, 'a')])
    monkeypatch.setattr(recheck, 'gh', fake)
    monkeypatch.setenv('GITHUB_REPOSITORY', 'o/r')
    monkeypatch.setattr(sys, 'argv', ['x', '--tag', 'cua-spaces-v1.0.0'])
    assert recheck.main() == 0
    assert fake.calls == []
    monkeypatch.setattr(sys, 'argv', ['x', '--tag', 'cua-spacesd-v0.16.3'])
    assert recheck.main() == 0
    assert ('POST', 'repos/o/r/actions/runs/1/rerun-failed-jobs') in fake.calls
    assert '#8174' in capsys.readouterr().out
