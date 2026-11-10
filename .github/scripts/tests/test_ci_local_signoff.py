"""The automatic local-ci sign-off on our self-hosted Mac runner never runs fork code (CUA-4001)."""
import json
import os
import subprocess
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / '.github/workflows'
HOOK = ROOT / '.github/runners/local-ci/job-started.sh'
SIGNOFF = yaml.safe_load((WORKFLOWS / 'ci-local-signoff.yml').read_text())
SAME_REPO = 'github.event.pull_request.head.repo.full_name == github.repository'


def test_only_pull_request_and_read_only_by_default():
    assert list(SIGNOFF[True]) == ['pull_request']
    assert SIGNOFF['permissions'] == {'contents': 'read'}


def test_self_hosted_job_is_same_repo_only_read_only_and_secret_free():
    job = SIGNOFF['jobs']['bin-ci']
    assert job['runs-on'] == ['cua-local-ci']
    assert job['if'] == SAME_REPO
    assert job['permissions'] == {'contents': 'read'}
    text = yaml.safe_dump(job)
    assert 'secrets.' not in text
    checkout = job['steps'][0]
    assert checkout['with']['persist-credentials'] is False
    run = next(s for s in job['steps'] if s.get('id') == 'run')
    assert run['run'].lstrip().splitlines()[2].startswith('set +e'), 'the verdict must be written even when a grep finds nothing'


def test_status_job_never_runs_pull_request_code():
    job = SIGNOFF['jobs']['sign-off']
    assert job['runs-on'] == 'ubuntu-latest' and SAME_REPO in job['if']
    for step in job['steps']:
        if str(step.get('uses', '')).startswith('actions/checkout'):
            assert 'default_branch' in step['with']['ref']


def test_no_other_workflow_targets_the_runner_label():
    users = [p.name for p in WORKFLOWS.glob('*.yml') if 'cua-local-ci' in p.read_text() and p.name != 'ci-local-signoff.yml']
    assert users == []


def test_gate_does_not_wait_for_the_sign_off_run():
    assert '/ci-local-signoff[.]yml($|@)' in (WORKFLOWS / 'ci-gate.yml').read_text()


def guard(tmp_path, event_name='pull_request', head='trycua/cua', base='trycua/cua',
          workflow='trycua/cua/.github/workflows/ci-local-signoff.yml@refs/pull/1/merge', repo='trycua/cua'):
    event = tmp_path / 'event.json'
    event.write_text(json.dumps({'pull_request': {'head': {'repo': {'full_name': head}}, 'base': {'repo': {'full_name': base}}}}))
    env = dict(os.environ, GITHUB_REPOSITORY=repo, GITHUB_EVENT_NAME=event_name, GITHUB_WORKFLOW_REF=workflow,
               GITHUB_EVENT_PATH=str(event))
    return subprocess.run(['bash', str(HOOK)], env=env, capture_output=True, text=True).returncode


def test_runner_guard_accepts_only_same_repo_sign_off_jobs(tmp_path):
    if not Path('/usr/bin/python3').exists():
        return
    assert guard(tmp_path) == 0
    assert guard(tmp_path, head='someone/cua') != 0
    assert guard(tmp_path, head='') != 0
    assert guard(tmp_path, event_name='pull_request_target') != 0
    assert guard(tmp_path, event_name='push') != 0
    assert guard(tmp_path, workflow='trycua/cua/.github/workflows/ci-quick.yml@refs/pull/1/merge') != 0
    assert guard(tmp_path, workflow='someone/cua/.github/workflows/ci-local-signoff.yml@refs/heads/main') != 0
    assert guard(tmp_path, repo='someone/cua') != 0
