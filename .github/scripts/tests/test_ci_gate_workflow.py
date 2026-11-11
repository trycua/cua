import json
import os
from pathlib import Path
import subprocess

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / '.github/workflows'
GATE = yaml.safe_load((WORKFLOWS / 'ci-gate.yml').read_text())
STEPS = {step.get('name'): step for step in GATE['jobs']['ci-gate']['steps']}
SCRIPT = STEPS['Wait for every other workflow on the pull request head']['run']


@pytest.mark.parametrize('filename', [
    'ci-contributor-attribution.yml',
    'ci-release-metadata.yml',
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
    # A wedged gate must not hold a required check for six hours. Three hours
    # still leaves headroom over the longest gate run observed on main.
    assert GATE['jobs']['ci-gate']['timeout-minutes'] == 180
    assert '.workflow_id != $GITHUB_WORKFLOW_ID' in SCRIPT
    assert 'list_runs "$sha" \'.event == "pull_request"\'' in SCRIPT
    assert 'list_runs "$queue_sha" \'.event == "merge_group" and .conclusion != "skipped"\'' in SCRIPT
    assert 'claude-auto-fix|ci-release-reminder|review-cua-perception-candidates|/cla[.]yml($|@)' in SCRIPT


def filtered_runs(ignore_re):
    runs_src = SCRIPT[SCRIPT.index('scope_jq='):]
    end = runs_src.index('\n}\n', runs_src.index('\nruns() {')) + len('\n}\n')  # list_runs() and runs()
    runs_src = runs_src[:end]
    paths = [
        ('.github/workflows/cla.yml', 'failure', 'completed'),
        ('.github/workflows/cla.yml@refs/pull/1/merge', '', 'in_progress'),
        ('.github/workflows/ci-contributor-attribution.yml', 'failure', 'completed'),
        ('.github/workflows/claude-auto-fix.yml', 'failure', 'completed'),
        ('.github/workflows/cla.yml.bak', 'success', 'completed'),
    ]
    runs_json = json.dumps({'workflow_runs': [
        {'workflow_id': i + 1, 'id': 100 + i, 'name': path.rsplit('/', 1)[1], 'path': path,
         'event': 'pull_request', 'conclusion': conclusion or None, 'status': status}
        for i, (path, conclusion, status) in enumerate(paths)
    ]})
    script = ('api() { jq -r "${@: -1}" <<<"$RUNS_JSON"; }\n' + runs_src + 'runs\n')
    out = subprocess.run(
        ['bash', '-e', '-c', script],
        env={**os.environ, 'RUNS_JSON': runs_json, 'IGNORE_RE': ignore_re,
             'GITHUB_WORKFLOW_ID': '0', 'sha': 'abc', 'queue_sha': '', 'GITHUB_REPOSITORY': 'test/repo'},
        capture_output=True, text=True, timeout=10, check=True,
    ).stdout
    return out.strip().splitlines()


@pytest.mark.parametrize('ignore_re', ['', 'unrelated-area[.]yml'])
def test_gate_never_waits_on_the_cla_workflow(ignore_re):
    rows = filtered_runs(ignore_re)
    assert [row.split('\t')[2] for row in rows] == [
        'ci-contributor-attribution.yml', 'cla.yml.bak',
    ]
    failing = run_gate('\n'.join(rows))
    assert failing.returncode == 1, failing.stderr
    assert 'ci-contributor-attribution.yml: fail' in failing.stdout
    passing = run_gate('\n'.join(
        row.replace('\tfailure\t', '\tsuccess\t') for row in rows
    ))
    assert passing.returncode == 0, passing.stderr


MOCK = '''
set -uo pipefail
api() {
  case "$*" in
    *'/check-runs?'*) printf 'check-runs\n' >&2; printf '%s' "${CHECK_RUNS:-}" ;;
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
             max_sleeps=4, max_seconds=10**9, queue_ref='', check_runs='', extra_env=None):
    # Mirrors the runner: GitHub runs a bare `run:` step with `bash -e`. The mocked
    # sleep advances SECONDS, so the poll schedule is observable without waiting.
    return subprocess.run(
        ['bash', '-e', '-c', MOCK + SCRIPT[SCRIPT.index('sha=$PR_SHA'):]],
        env={**os.environ, 'PR_SHA': 'abc', 'QUEUE_REF': queue_ref,
             'GITHUB_REPOSITORY': 'test/repo', 'GITHUB_RUN_ID': '99',
             'ROWS': rows, 'JOBS': jobs,
             'OLDER_JOBS': jobs if older_jobs is None else older_jobs,
             'ROWS_DIR': str(rows_dir) if rows_dir else '',
             'MAX_SLEEPS': str(max_sleeps), 'MAX_SECONDS': str(max_seconds),
             'CHECK_RUNS': check_runs, **(extra_env or {})},
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
    # A scoped macOS sample build: the scope job ran, the sample job was skipped
    # (the jobs API reports it as skipped and the gate ignores it), so the run passes.
    ('1\t11\tinfinite-canvas\tsuccess\tcompleted', 'scope (macOS Swift sample)\tsuccess', None, 0, 'infinite-canvas: pass'),
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


def test_gate_reads_the_pull_request_number_from_a_merge_queue_ref():
    ref = 'refs/heads/gh-readonly-queue/main/pr-5811-0123abcd'
    completed = run_gate(DONE, queue_ref=ref)
    assert completed.returncode == 0, completed.stderr
    assert 'build: pass' in completed.stdout


def test_gate_fails_on_a_merge_queue_ref_without_a_pull_request_number():
    completed = run_gate(DONE, queue_ref='refs/heads/gh-readonly-queue/main/odd')
    assert completed.returncode == 1
    assert 'build: pass' not in completed.stdout


QUARANTINE_MOCK = MOCK.replace("    *) printf '999' ;;", """    *'--jq .run_attempt'*) cat "$STATE/attempt" ;;
    *) printf '999' ;;""").replace("""    *'head_sha='*)""", """    *'head_sha='*)
      if [[ -f "$STATE/reran" ]]; then printf '%s' "$ROWS_AFTER"; return; fi""") + '''
gh() { if [[ "$*" == *rerun-failed-jobs* ]]; then touch "$STATE/reran"; echo 2 > "$STATE/attempt"; else return 1; fi; }
'''


def run_quarantine_gate(tmp_path, check, attempt='1', after='1\t11\tbuild\tsuccess\tcompleted'):
    stub = tmp_path / 'flaky.py'
    stub.write_text(f'import sys; sys.stdin.read(); print({check!r})\n')
    (tmp_path / 'attempt').write_text(attempt)
    return subprocess.run(
        ['bash', '-e', '-c', QUARANTINE_MOCK + SCRIPT[SCRIPT.index('sha=$PR_SHA'):]],
        env={**os.environ, 'PR_SHA': 'abc', 'QUEUE_REF': '', 'GITHUB_REPOSITORY': 'test/repo',
             'GITHUB_RUN_ID': '99', 'ROWS': '1\t11\tbuild\tfailure\tcompleted', 'ROWS_AFTER': after,
             'JOBS': '5\tlinux-kvm (concurrent)', 'OLDER_JOBS': '', 'ROWS_DIR': '', 'STATE': str(tmp_path),
             'FLAKY_PY': str(stub), 'QUARANTINE_FILE': 'q.json', 'MAX_SLEEPS': '12', 'MAX_SECONDS': str(10**9)},
        capture_output=True, text=True, timeout=20,
    )


def test_gate_reruns_a_quarantined_flake_once_and_passes_when_it_passes(tmp_path):
    done = run_quarantine_gate(tmp_path, 'all')
    assert done.returncode == 0, done.stderr
    assert (tmp_path / 'reran').exists()
    assert 'rerunning its failed jobs once' in done.stdout
    assert done.stdout.strip().splitlines()[-1] == 'build: pass'


def test_gate_does_not_fail_on_a_quarantined_flake_that_fails_again(tmp_path):
    done = run_quarantine_gate(tmp_path, 'all', after='1\t11\tbuild\tfailure\tcompleted')
    assert done.returncode == 0, done.stderr
    assert 'build: pass (quarantined flake' in done.stdout


def test_gate_fails_a_failure_that_is_not_quarantined(tmp_path):
    done = run_quarantine_gate(tmp_path, 'none')
    assert done.returncode == 1
    assert not (tmp_path / 'reran').exists()
    assert 'build: fail' in done.stdout


def test_gate_reads_the_quarantine_list_from_the_base_commit():
    base = STEPS['Check out the selector from the base commit']['with']['sparse-checkout'].split()
    assert '/.github/flaky/quarantine.json' in base and '/.github/scripts/flaky/flaky.py' in base
    env = STEPS['Wait for every other workflow on the pull request head']['env']
    assert env['FLAKY_PY'].startswith('base/') and env['QUARANTINE_FILE'].startswith('base/')


def test_gate_fails_a_run_whose_quick_job_failed_and_skipped_the_rest():
    # `quick` failed; the heavy jobs were skipped (the jobs API lists them, the gate drops them).
    # The run is a failure, so the gate must not read the skipped jobs as a pass.
    completed = run_gate('1\t11\tbuild\tfailure\tcompleted', jobs='quick / quick\tfailure')
    assert completed.returncode == 1
    assert 'build: fail' in completed.stdout


HEAVY_WORKFLOWS = sorted(
    path for path in WORKFLOWS.glob('*.yml')
    if 'quick' in (yaml.safe_load(path.read_text()).get('jobs') or {})
    and path.name != 'ci-quick.yml'
)


def test_quick_stage_is_wired_into_heavy_workflows():
    assert HEAVY_WORKFLOWS
    for path in HEAVY_WORKFLOWS:
        workflow = yaml.safe_load(path.read_text())
        jobs = workflow['jobs']
        assert jobs['quick']['uses'] == './.github/workflows/ci-quick.yml', path.name
        # The stage starts on pull requests and merge queue entries (CUA-3856).
        assert 'merge_group' in workflow[True], path.name
        assert jobs['quick']['if'] == "(github.event_name == 'pull_request' || github.event_name == 'merge_group')"
        for name, job in jobs.items():
            if name == 'quick':
                continue
            needs = job.get('needs', [])
            needs = [needs] if isinstance(needs, str) else needs
            # Every job waits for `quick` directly, so it can read the scope decision: a job that only
            # waited through another job could still start (`!cancelled()`) when that job was skipped.
            assert 'quick' in needs, f'{path.name}: {name} does not wait for quick'
            assert "needs.quick.outputs.skip != 'true'" in str(job['if']), f'{path.name}: {name} ignores the scope'
            # Still runs on push and manual runs, where `quick` is skipped: a status function other than
            # the implicit success() decides.
            assert any(f in str(job['if']) for f in ('!cancelled()', 'always()', 'failure()')), f'{path.name}: {name}'


def test_quick_stage_decides_the_scope_from_the_base_commit_and_fails_closed():
    quick = yaml.safe_load((WORKFLOWS / 'ci-quick.yml').read_text())
    assert quick[True]['workflow_call']['outputs']['skip']['value'] == '${{ jobs.scope.outputs.skip }}'
    scope = quick['jobs']['scope']
    steps = {s.get('name'): s for s in scope['steps']}
    base = steps['Check out the selector from the base commit']['with']
    assert base['ref'] == '${{ github.event.pull_request.base.sha || github.event.merge_group.base_sha }}'
    assert '/.github/ci-areas.json' in base['sparse-checkout']
    decide = steps['Decide whether this workflow runs']['run']
    assert 'set -euo pipefail' in decide
    assert 'python3 base/.github/scripts/ci_gate_scope.py --workflow' in decide
    # Merge queue runs stay off (and ci-gate reads the pull request's results) until the switch is on.
    assert '"${QUEUE_RUNS:-}" != true' in decide and 'skip=true' in decide


LOCAL_CI_FAST = STEPS['Check the sign-off on the pull request (fail fast)']['run']
LOCAL_CI_WAIT = STEPS['Require the sign-off on the pull request']['run']
# The stub answers every sign-off lookup with STATE: a commit status for a branch of this repository,
# the newest CI: Quick run for a fork. SOURCE records which one the gate asked.
GH_STUB = """
sleep() { :; }
gh() {
  case "$*" in
    *'/pulls/'*) printf '%s\\t%s\\t%s\\t%s\\n' "deadbeefcafe" "$HEAD_REPO" "${HEAD_REF:-feat/x}" "${PR_LABELS:-}" ;;
    *'/statuses'*) echo statuses >> "$SOURCE"; printf '%s\\n' "$STATE" ;;
    *'/workflows/ci-quick.yml/runs'*) echo quick-run >> "$SOURCE"; printf '%s\\n' "$STATE" ;;
  esac
}
"""


def run_local_ci(head_repo='test/repo', state='', require='true', head_ref='feat/x', pr_labels=''):
    """Run the two sign-off steps in order, as the job does: the fast check, then the wait that is
    the last step. Returns the first failure, or the last result; `.source` is what was asked."""
    import tempfile
    with tempfile.NamedTemporaryFile('r+') as out, tempfile.NamedTemporaryFile('r+') as source:
        env = {**os.environ, 'PR_NUMBER': '7', 'QUEUE_REF': '', 'GITHUB_REPOSITORY': 'test/repo',
               'HEAD_REPO': head_repo, 'HEAD_REF': head_ref, 'PR_LABELS': pr_labels, 'STATE': state, 'SOURCE': source.name,
               'REQUIRE_LOCAL_CI': require, 'GITHUB_OUTPUT': out.name}
        fast = subprocess.run(['bash', '-e', '-c', GH_STUB + LOCAL_CI_FAST], env=env,
                              capture_output=True, text=True, timeout=20)
        outputs = dict(line.split('=', 1) for line in out.read().splitlines())
        if fast.returncode != 0:
            fast.source = source.read().split()
            return fast
        wait = subprocess.run(['bash', '-e', '-c', GH_STUB + LOCAL_CI_WAIT],
                              env={**env, 'SHA': outputs['sha'], 'KIND': outputs['kind']},
                              capture_output=True, text=True, timeout=20)
        wait.stdout = fast.stdout + wait.stdout
        wait.source = source.read().split()
    return wait


def test_every_pull_request_needs_a_sign_off_whoever_opens_it():
    # The author, the labels and the commit trailers are not read: a branch of this repository
    # always needs the local-ci status, a fork always needs the CI: Quick run.
    assert 'author' not in LOCAL_CI_FAST.lower().replace('author:', '')
    for word in ('trailer', 'agent/'):
        assert word not in LOCAL_CI_FAST
        assert word not in LOCAL_CI_WAIT


RP_REF = 'release-please--branches--main--components--cua-spaces'


def test_release_please_pull_request_is_exempt_from_the_sign_off():
    ok = run_local_ci(head_ref=RP_REF, pr_labels='autorelease: pending')
    assert ok.returncode == 0 and ok.source == []
    # Branch prefix, label and a branch of this repository are all required.
    for kwargs in ({'head_ref': RP_REF, 'pr_labels': 'other'},
                   {'head_ref': 'feat/x', 'pr_labels': 'autorelease: pending'},
                   {'head_ref': RP_REF, 'pr_labels': 'autorelease: pending', 'head_repo': 'fork/repo'}):
        assert run_local_ci(**kwargs).returncode == 1, kwargs


def test_branch_of_this_repo_needs_a_local_ci_status():
    missing = run_local_ci()
    assert missing.returncode == 1
    assert 'Run bin/ci --quick and bin/ci --signoff on this head commit.' in missing.stdout
    assert set(missing.source) == {'statuses'}
    assert run_local_ci(state='failure').returncode == 1
    assert run_local_ci(state='success').returncode == 0


def test_fork_needs_a_successful_quick_run_in_github_actions():
    missing = run_local_ci(head_repo='someone/fork')
    assert missing.returncode == 1
    assert 'no successful run on this head commit' in missing.stdout
    assert set(missing.source) == {'quick-run'}  # never the local-ci status, which only our Macs set
    assert run_local_ci(head_repo='someone/fork', state='failure').returncode == 1
    assert run_local_ci(head_repo='someone/fork', state='success').returncode == 0


def test_sign_off_is_report_only_until_the_switch_is_on():
    for head_repo in ('test/repo', 'someone/fork'):
        for state in ('', 'failure'):
            done = run_local_ci(head_repo=head_repo, state=state, require='')
            assert done.returncode == 0, done.stdout
            assert '::warning' in done.stdout
        assert run_local_ci(head_repo=head_repo, state='success', require='').returncode == 0


def test_the_quick_workflow_a_fork_relies_on_is_safe_to_run_on_its_code():
    wf = yaml.safe_load((WORKFLOWS / 'ci-quick.yml').read_text())
    triggers = wf.get(True) or wf.get('on')
    assert 'pull_request' in triggers  # runs on fork PRs, GitHub-hosted
    assert 'pull_request_target' not in triggers
    assert wf['permissions'] == {'contents': 'read'}
    text = (WORKFLOWS / 'ci-quick.yml').read_text()
    assert 'secrets.' not in text
    assert 'self-hosted' not in text


def test_quick_stage_runs_once_per_commit():
    steps = yaml.safe_load((WORKFLOWS / 'ci-quick.yml').read_text())['jobs']['quick']['steps']
    restore, *middle, save = [s for s in steps if s.get('id') == 'marker' or 'if' in s or 'cache' in str(s.get('uses', ''))]
    key = 'ci-quick-pass-${{ github.sha }}'
    assert restore['uses'].startswith('actions/cache/restore@') and restore['with']['key'] == key
    assert save['uses'].startswith('actions/cache/save@') and save['with']['key'] == key
    assert steps[0] is restore
    # Every step after the restore is skipped on a hit; the save runs last, after the checks passed
    # (a failing step stops the job before it, so a failure is never cached).
    for step in steps[1:]:
        assert step['if'] == "steps.marker.outputs.cache-hit != 'true'", step.get('name', step.get('uses'))
    assert steps[-1] is save
    assert any('bin/ci --quick' in str(s.get('run', '')) for s in middle)


# Merge queue runs (CUA-3856): the listing for the queue commit answers QROWS, the pull request head ROWS.
QUEUE_MOCK = MOCK.replace("""    *'head_sha='*)""", """    *'head_sha=queue'*) printf '%s' "$QROWS" ;;
    *'head_sha='*)""")


def run_queue_gate(rows, qrows, queue_runs, jobs='test\tsuccess', check_runs=''):
    return subprocess.run(
        ['bash', '-e', '-c', QUEUE_MOCK + SCRIPT[SCRIPT.index('sha=$PR_SHA'):]],
        env={**os.environ, 'PR_SHA': '', 'QUEUE_REF': 'refs/heads/gh-readonly-queue/main/pr-7-abc',
             'QUEUE_SHA': 'queue', 'QUEUE_RUNS': queue_runs, 'GITHUB_REPOSITORY': 'test/repo',
             'GITHUB_RUN_ID': '99', 'ROWS': rows, 'QROWS': qrows, 'JOBS': jobs, 'OLDER_JOBS': jobs,
             'ROWS_DIR': '', 'MAX_SLEEPS': '6', 'MAX_SECONDS': str(10**9), 'CHECK_RUNS': check_runs},
        capture_output=True, text=True, timeout=10,
    )


def test_merge_queue_judges_a_workflow_by_its_queue_run_when_queue_runs_are_on():
    pr = '1\t11\tbuild\tsuccess\tcompleted\n2\t21\tlint\tsuccess\tcompleted'
    queue = '1\t12\tbuild\tfailure\tcompleted'
    on = run_queue_gate(pr, queue, 'true', jobs='test\tfailure')
    assert on.returncode == 1 and 'build: fail' in on.stdout
    # `lint` has no queue run, so its pull request result still counts.
    assert 'lint:' in on.stdout
    off = run_queue_gate(pr, queue, '')
    assert off.returncode == 0 and 'build: pass' in off.stdout


def test_merge_queue_passes_on_green_queue_runs():
    completed = run_queue_gate('1\t11\tbuild\tsuccess\tcompleted', '1\t12\tbuild\tsuccess\tcompleted', 'true')
    assert completed.returncode == 0, completed.stderr
    assert 'build: pass' in completed.stdout


# Fail fast (10 Nov 2026): a job that already failed in a run that is still going fails the gate at once.
# The check-runs listing answers CHECK_RUNS as "<run id>\t<job id>\t<job name>" rows (the jq is mocked away).
STILL_GOING = '1\t11\tbuild\tsuccess\tcompleted\n2\t21\tmacOS\t\tin_progress'


def test_gate_fails_at_once_on_a_failed_job_in_a_run_that_is_still_going():
    completed = run_gate(STILL_GOING, check_runs='21\t501\tapp core + FFI mirror gate', max_sleeps=100)
    assert completed.returncode == 1, completed.stderr
    assert 'macOS: fail (job app core + FFI mirror gate failed' in completed.stdout
    # The first poll after the minimum window decides; no waiting for the macOS run to finish.
    assert sleeps_of(completed) == [MIN_WINDOW]


def test_merge_queue_gate_fails_at_once_on_the_pull_request_runs_failed_job():
    # #7136: QUEUE_RUNS off, so the queue entry is judged by the pull request's own runs; one of them
    # had a failed job and a 180-minute job still running. The entry must leave the queue now.
    completed = run_queue_gate(STILL_GOING, '', '', check_runs='21\t501\tapp core + FFI mirror gate')
    assert completed.returncode == 1, completed.stderr
    assert 'macOS: fail' in completed.stdout


@pytest.mark.parametrize('check_runs', [
    '',                                  # nothing failed yet
    '11\t401\tlint',                    # a completed run: judged by the full verdict, not here
    '77\t701\tunrelated',               # a run the gate does not wait on (another scope, an older run)
])
def test_gate_keeps_waiting_without_a_failed_job_in_a_run_it_waits_on(check_runs):
    completed = run_gate(STILL_GOING, check_runs=check_runs, max_sleeps=6)
    assert completed.returncode == 124
    assert ': fail' not in completed.stdout


def test_gate_only_reads_the_newest_run_of_a_workflow_that_is_still_going():
    rows = '2\t20\tmacOS\t\tin_progress\n2\t21\tmacOS\t\tin_progress'
    old = run_gate(rows, check_runs='20\t500\tbuild', max_sleeps=6)
    assert old.returncode == 124 and ': fail' not in old.stdout
    new = run_gate(rows, check_runs='21\t501\tbuild', max_sleeps=6)
    assert new.returncode == 1 and 'macOS: fail' in new.stdout


def test_gate_waits_for_the_full_verdict_on_a_quarantined_early_failure(tmp_path):
    stub = tmp_path / 'flaky.py'
    stub.write_text("import sys; sys.stdin.read(); print('all')\n")
    completed = run_gate(STILL_GOING, check_runs='21\t501\tlinux-kvm (concurrent)', max_sleeps=6,
                         extra_env={'FLAKY_PY': str(stub), 'QUARANTINE_FILE': 'q.json'})
    assert completed.returncode == 124
    assert ': fail' not in completed.stdout


def test_fail_fast_reads_check_runs_at_most_once_a_minute():
    completed = run_gate(STILL_GOING, max_sleeps=1000, max_seconds=3600)
    assert completed.returncode == 124
    reads = completed.stderr.count('check-runs')
    assert 50 <= reads <= 61


def test_gate_may_read_check_runs():
    assert GATE['permissions']['checks'] == 'read'
