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
        file=
        for candidate in "$ROWS_DIR"/rows.*; do
          at=${candidate##*rows.}
          if (( at <= SECONDS )) && [[ -z "$file" || ${file##*rows.} -lt $at ]]; then file=$candidate; fi
        done
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


def timeline(tmp_path, snapshots):
    """Run listings by simulated second: {0: rows, 125: rows} shows `rows` from 125 s on."""
    for at, rows in snapshots.items():
        (tmp_path / f'rows.{at}').write_text(rows)
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
LATE_FAILURE = DONE + '\n2\t12\tlint\tfailure\tcompleted'
MIN_WINDOW = 120


def test_gate_gives_no_verdict_before_the_minimum_window():
    completed = run_gate(DONE)
    assert completed.returncode == 0, completed.stderr
    sleeps = sleeps_of(completed)
    assert sleeps[0] >= MIN_WINDOW
    assert sleeps == [MIN_WINDOW, 10]


def test_gate_catches_a_failing_run_that_appears_inside_the_window(tmp_path):
    # Identical completed listings at 30 s and 40 s would have passed this PR; the
    # failing workflow only shows up at 45 s. No poll may count before the window ends.
    rows = timeline(tmp_path, {0: DONE, 45: LATE_FAILURE})
    completed = run_gate('', rows_dir=rows)
    assert completed.returncode == 1, completed.stderr
    assert sleeps_of(completed) == [MIN_WINDOW, 10]
    assert 'build: pass' in completed.stdout
    assert 'lint: fail' in completed.stdout


def test_gate_needs_two_identical_completed_polls(tmp_path):
    rows = timeline(tmp_path, {0: '1\t11\tbuild\t\tin_progress', 125: DONE})
    completed = run_gate('', rows_dir=rows)
    assert completed.returncode == 0, completed.stderr
    assert sleeps_of(completed) == [MIN_WINDOW, 10, 10]
    assert 'build: pass' in completed.stdout


def test_gate_holds_for_a_run_that_appears_after_the_first_poll(tmp_path):
    rows = timeline(tmp_path, {0: DONE, 125: LATE_FAILURE})
    completed = run_gate('', rows_dir=rows)
    assert completed.returncode == 1, completed.stderr
    assert sleeps_of(completed) == [MIN_WINDOW, 10, 10]
    assert 'build: pass' in completed.stdout
    assert 'lint: fail' in completed.stdout


def test_gate_poll_schedule_backs_off_and_stays_within_the_api_budget():
    stuck = '1\t11\tbuild\t\tin_progress'
    completed = run_gate(stuck, max_sleeps=1000, max_seconds=3600)
    assert completed.returncode == 124
    sleeps = sleeps_of(completed)
    assert sleeps[0] == MIN_WINDOW
    intervals = sleeps[1:]
    assert intervals == sorted(intervals)
    assert set(intervals) == {10, 30, 60}
    # Fast polling only while quick PRs finish, then 30 s, then one poll a minute.
    assert intervals.count(10) == 18
    assert intervals.count(30) == 20
    # One run listing is one or two API calls; keep a long suite under 100 an hour.
    assert len(sleeps) <= 100
