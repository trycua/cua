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


MOCK = '''
set -uo pipefail
api() {
  case "$*" in
    *'/jobs?'*)
      if [[ "$*" == *'/10/jobs?'* ]]; then
        printf '%s' "$OLDER_JOBS"
      else
        printf '%s' "$JOBS"
      fi ;;
    *'head_sha='*)
      if [[ -n "${ROWS_DIR:-}" ]]; then
        n=$(( $(cat "$ROWS_DIR/polls" 2>/dev/null || echo 0) + 1 ))
        echo "$n" > "$ROWS_DIR/polls"
        file="$ROWS_DIR/rows.$n"
        [[ -f "$file" ]] || file="$ROWS_DIR/rows.last"
        cat "$file"
      else
        printf '%s' "$ROWS"
      fi ;;
    *) printf '999' ;;
  esac
}
sleeps=0
sleep() {
  sleeps=$((sleeps + 1))
  printf 'sleep %s\\n' "$1" >&2
  SECONDS=$((SECONDS + $1))
  if (( sleeps > MAX_SLEEPS || SECONDS > MAX_SECONDS )); then exit 124; fi
}
'''


def run_gate(rows, jobs='test\tsuccess', older_jobs=None, rows_dir=None,
             max_sleeps=4, max_seconds=10**9):
    # Mirrors the runner: GitHub runs a bare `run:` step with `bash -e`. The mocked
    # sleep advances SECONDS, so the poll schedule is observable without waiting.
    return subprocess.run(
        ['bash', '-e', '-c', MOCK + SCRIPT[SCRIPT.index('sha=$PR_SHA'):]],
        env={**os.environ, 'PR_SHA': 'abc', 'QUEUE_REF': '',
             'GITHUB_REPOSITORY': 'test/repo', 'GITHUB_RUN_ID': '99',
             'ROWS': rows, 'JOBS': jobs,
             'OLDER_JOBS': jobs if older_jobs is None else older_jobs,
             'ROWS_DIR': str(rows_dir) if rows_dir else '',
             'MAX_SLEEPS': str(max_sleeps), 'MAX_SECONDS': str(max_seconds)},
        capture_output=True, text=True, timeout=10,
    )


def sleeps_of(completed):
    return [int(line.split()[1]) for line in completed.stderr.splitlines()
            if line.startswith('sleep ')]


def poll_sequence(tmp_path, *polls):
    for number, rows in enumerate(polls, 1):
        (tmp_path / f'rows.{number}').write_text(rows)
    (tmp_path / 'rows.last').write_text(polls[-1])
    return tmp_path


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


DONE = '1\t11\tbuild\tsuccess\tcompleted'


def test_gate_finishes_within_a_minute_when_every_run_is_done():
    completed = run_gate(DONE)
    assert completed.returncode == 0, completed.stderr
    assert sleeps_of(completed) == [30, 10]


def test_gate_needs_two_identical_completed_polls(tmp_path):
    rows = poll_sequence(
        tmp_path,
        '1\t11\tbuild\t\tin_progress',
        DONE,
        DONE,
    )
    completed = run_gate('', rows_dir=rows)
    assert completed.returncode == 0, completed.stderr
    assert sleeps_of(completed) == [30, 10, 10]
    assert 'build: pass' in completed.stdout


def test_gate_holds_for_a_run_that_appears_after_the_first_poll(tmp_path):
    late_failure = DONE + '\n2\t12\tlint\tfailure\tcompleted'
    rows = poll_sequence(tmp_path, DONE, late_failure, late_failure)
    completed = run_gate('', rows_dir=rows)
    assert completed.returncode == 1, completed.stderr
    assert sleeps_of(completed) == [30, 10, 10]
    assert 'build: pass' in completed.stdout
    assert 'lint: fail' in completed.stdout


def test_gate_poll_schedule_backs_off_and_stays_within_the_api_budget():
    stuck = '1\t11\tbuild\t\tin_progress'
    completed = run_gate(stuck, max_sleeps=1000, max_seconds=3600)
    assert completed.returncode == 124
    sleeps = sleeps_of(completed)
    assert sleeps[0] == 30
    intervals = sleeps[1:]
    assert intervals == sorted(intervals)
    assert set(intervals) == {10, 30, 60}
    # Fast polling only while quick PRs finish, then 30 s, then one poll a minute.
    assert intervals.count(10) == 27
    assert intervals.count(30) == 20
    # One run listing is one or two API calls; keep a long suite near 100 an hour.
    assert len(sleeps) <= 100
