import os
from pathlib import Path
import subprocess

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / '.github/workflows'
GATE = yaml.safe_load((WORKFLOWS / 'ci-gate.yml').read_text())
SCRIPT = GATE['jobs']['ci-gate']['steps'][0]['run']


@pytest.mark.parametrize('filename', [
    'ci-contributor-attribution.yml',
    'ci-release-metadata.yml',
    'fleet-mirror-guard.yml',
])
def test_required_workflow_queue_wiring(filename):
    workflow = yaml.safe_load((WORKFLOWS / filename).read_text())
    assert 'merge_group' in workflow[True]
    for job in workflow['jobs'].values():
        assert job['if'] in (
            "github.event_name != 'merge_group'",
            "always() && github.event_name != 'merge_group'",
        )


def test_gate_wiring():
    assert set(GATE[True]) == {'pull_request', 'merge_group'}
    assert GATE[True]['pull_request']['types'] == [
        'opened', 'synchronize', 'reopened', 'ready_for_review',
        'edited', 'labeled', 'unlabeled',
    ]
    assert 'concurrency' not in GATE
    assert GATE['jobs']['ci-gate']['timeout-minutes'] == 360
    assert '.workflow_id != $GITHUB_WORKFLOW_ID' in SCRIPT
    assert '(.event == \\"pull_request\\" or .event == \\"pull_request_target\\")' in SCRIPT
    assert 'claude-auto-fix|ci-release-reminder|review-cua-perception-candidates' in SCRIPT


def run_gate(rows, jobs='test\tsuccess', older_jobs=None):
    mock = '''
set -uo pipefail
api() {
  case "$*" in
    *'/jobs?'*)
      if [[ "$*" == *'/10/jobs?'* ]]; then
        printf '%s' "$OLDER_JOBS"
      else
        printf '%s' "$JOBS"
      fi ;;
    *'head_sha='*) printf '%s' "$ROWS" ;;
    *) printf '999' ;;
  esac
}
sleeps=0
sleep() {
  sleeps=$((sleeps + 1))
  if (( sleeps > 4 )); then exit 124; fi
}
'''
    return subprocess.run(
        ['bash', '-c', mock + SCRIPT[SCRIPT.index('sha=$PR_SHA'):]],
        env={**os.environ, 'PR_SHA': 'abc', 'QUEUE_REF': '',
             'GITHUB_REPOSITORY': 'test/repo', 'GITHUB_RUN_ID': '99',
             'ROWS': rows, 'JOBS': jobs,
             'OLDER_JOBS': jobs if older_jobs is None else older_jobs},
        capture_output=True, text=True, timeout=5,
    )


@pytest.mark.parametrize(('rows', 'jobs', 'older_jobs', 'code', 'result'), [
    ('1\t11\tbuild\tsuccess\tcompleted\n1\t10\tbuild\tfailure\tcompleted', 'test\tsuccess', None, 0, 'build: pass'),
    ('1\t11\tbuild\tsuccess\tcompleted\n1\t10\tbuild\tcancelled\tcompleted', '', 'test\tcancelled', 0, 'build: skipped'),
    ('1\t11\tbuild\tcancelled\tcompleted\n1\t10\tbuild\tsuccess\tcompleted', 'test\tcancelled', None, 1, 'build: fail'),
    ('1\t11\tbuild\tsuccess\tcompleted\n1\t10\tbuild\tfailure\tcompleted', '', 'test\tfailure', 1, 'build: fail'),
    ('1\t11\tbuild\tskipped\tcompleted', '', None, 0, 'build: skipped'),
    ('1\t11\tbuild\tsuccess\tcompleted', 'distro-smoke\tfailure', None, 0, 'build: pass'),
    ('1\t11\tbuild\tfailure\tcompleted', 'test\tfailure', None, 1, 'build: fail'),
    ('1\t11\tbuild\tneutral\tcompleted', 'test\tneutral', None, 0, 'build: pass'),
])
def test_gate_results(rows, jobs, older_jobs, code, result):
    completed = run_gate(rows, jobs, older_jobs)
    assert completed.returncode == code, completed.stderr
    assert result in completed.stdout


@pytest.mark.parametrize('rows', ['', '1\t11\tbuild\t\tin_progress'])
def test_gate_waits_until_timeout_without_completed_runs(rows):
    completed = run_gate(rows)
    assert completed.returncode == 124
    assert 'build: pass' not in completed.stdout
