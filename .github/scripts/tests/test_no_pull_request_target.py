"""GitHub blocks pull_request_target by default on public repositories (enforced 2 Dec 2026).

No workflow may use it. Fork pull requests are handled with pull_request (read-only token, no
secrets) plus a workflow_run workflow that reads an artifact as untrusted data.
"""
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / '.github/workflows'
sys.path.insert(0, str(ROOT / '.github/scripts'))
import cla_pr_report as cla  # noqa: E402


def triggers(path):
    on = yaml.safe_load(path.read_text()).get(True) or {}
    return [on] if isinstance(on, str) else list(on)


def test_no_workflow_uses_pull_request_target():
    offenders = [p.name for p in WORKFLOWS.glob('*.yml') if 'pull_request_target' in triggers(p)]
    assert offenders == []


def test_privileged_cla_report_never_checks_out_pull_request_code():
    wf = yaml.safe_load((WORKFLOWS / 'cla-report.yml').read_text())
    assert list(wf[True]) == ['workflow_run']
    for step in wf['jobs']['report']['steps']:
        if str(step.get('uses', '')).startswith('actions/checkout'):
            assert 'ref' not in step.get('with', {}) and 'repository' not in step.get('with', {})


def test_attribution_runs_read_only_on_pull_request():
    wf = yaml.safe_load((WORKFLOWS / 'ci-contributor-attribution.yml').read_text())
    assert 'pull_request' in wf[True]
    assert wf['permissions'] == {'contents': 'read', 'pull-requests': 'read'}
    assert '${{ secrets.' not in (WORKFLOWS / 'ci-contributor-attribution.yml').read_text()


def test_number_from_artifact_is_validated():
    assert cla.parse_number('123\n') == 123
    for bad in ('', '0', '12a', '../1', '1 2', '-1', '1' * 12):
        try:
            cla.parse_number(bad)
        except SystemExit:
            continue
        raise AssertionError(bad)


class FakeClient:
    def __init__(self, pull, comments=()):
        self._pull, self.comments, self.calls = pull, list(comments), []

    def pull(self, repository, number):
        return self._pull

    def get(self, path):
        return self.comments

    def send(self, method, path, payload):
        self.calls.append((method, path.split('/', 3)[3], payload))
        return {}


def pull(**over):
    base = {'state': 'open', 'author_association': 'NONE', 'user': {'type': 'User', 'login': 'x'},
            'head': {'sha': 'abc'}, 'number': 5}
    return {**base, **over}


def test_member_gets_success_status_and_no_comment():
    client = FakeClient(pull(author_association='MEMBER'))
    assert cla.run(client, 'o/r', 5, 'abc', Path('x')) == 0
    assert [(m, p) for m, p, _ in client.calls] == [('POST', 'statuses/abc')]
    assert client.calls[0][2]['state'] == 'success'


def test_stale_head_is_skipped():
    client = FakeClient(pull())
    cla.run(client, 'o/r', 5, 'old', Path('x'))
    assert client.calls == []


def test_unsigned_author_gets_failure_status_and_one_comment(monkeypatch):
    monkeypatch.setattr(cla, 'unsigned_for', lambda *a: ['@x has not signed the CLA; sign it on #5'])
    client = FakeClient(pull())
    cla.run(client, 'o/r', 5, 'abc', Path('x'))
    kinds = [(m, p.split('/')[0]) for m, p, _ in client.calls]
    assert kinds == [('POST', 'statuses'), ('POST', 'issues')]
    assert client.calls[0][2]['state'] == 'failure'
    assert cla.SIGN_PHRASE in client.calls[1][2]['body']


def test_signed_author_clears_the_old_comment(monkeypatch):
    monkeypatch.setattr(cla, 'unsigned_for', lambda *a: [])
    client = FakeClient(pull(), comments=[{'id': 9, 'body': cla.MARKER + ' old'}])
    cla.run(client, 'o/r', 5, 'abc', Path('x'))
    assert client.calls[0][2]['state'] == 'success'
    assert client.calls[1][:2] == ('DELETE', 'issues/comments/9')
