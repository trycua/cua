"""AWS roles are only assumed from trusted, non-pull-request contexts.

Every AWS role this repository assumes trusts only OIDC subjects for the main branch or a
protected environment, never the pull request subject. These checks keep the workflows
consistent with that, so pull request runs never hold an OIDC token they do not need:

- a workflow triggered by pull_request never grants `id-token: write` at the workflow level;
  jobs that need it ask for it themselves;
- in a workflow triggered by pull_request, every step that assumes an AWS role is guarded (on
  the step or its job) so it never runs for a pull_request event;
- workflows triggered by events that outside contributors can cause and that run in the
  default-branch context (issue_comment, workflow_run, pull_request_target and friends)
  never assume an AWS role.
"""
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / '.github/workflows'

PR_EVENTS = {'pull_request', 'pull_request_target'}
DEFAULT_BRANCH_CONTEXT_EVENTS = {
    'pull_request_target', 'issue_comment', 'issues', 'pull_request_review',
    'pull_request_review_comment', 'discussion', 'discussion_comment', 'workflow_run',
}
AWS_ACTIONS = ('aws-actions/configure-aws-credentials', './.github/actions/publish-downloads-cdn')
PR_GUARDS = (
    "github.event_name != 'pull_request'",
    "github.event_name == 'push'",
    "github.event_name == 'workflow_dispatch'",
    "github.event_name == 'schedule'",
)


def load(path):
    return yaml.safe_load(path.read_text())


def triggers(wf):
    on = wf.get(True) or wf.get('on') or {}
    return {on} if isinstance(on, str) else set(on)


def workflows():
    return sorted(WORKFLOWS.glob('*.yml')) + sorted(WORKFLOWS.glob('*.yaml'))


def aws_steps(wf):
    for job_id, job in (wf.get('jobs') or {}).items():
        for step in job.get('steps') or []:
            uses = str(step.get('uses', ''))
            if uses.startswith(AWS_ACTIONS) or 'role-to-assume' in (step.get('with') or {}):
                yield job_id, job, step


def grants_id_token(perms):
    return isinstance(perms, dict) and perms.get('id-token') == 'write' or perms == 'write-all'


def guarded(cond):
    return any(g in str(cond or '') for g in PR_GUARDS)


def test_detector_finds_aws_steps():
    names = {p.name for p in workflows() if any(aws_steps(load(p)))}
    assert 'docs-mcp-server-build-push.yml' in names


def test_pull_request_workflows_do_not_grant_id_token_at_workflow_level():
    offenders = [
        p.name for p in workflows()
        if triggers(load(p)) & PR_EVENTS and grants_id_token(load(p).get('permissions'))
    ]
    assert offenders == []


def test_aws_role_steps_never_run_for_pull_request_events():
    offenders = []
    for p in workflows():
        wf = load(p)
        if not triggers(wf) & PR_EVENTS:
            continue
        for job_id, job, step in aws_steps(wf):
            if not (guarded(job.get('if')) or guarded(step.get('if'))):
                offenders.append(f'{p.name}:{job_id}:{step.get("name", step.get("uses"))}')
    assert offenders == []


def test_jobs_that_assume_aws_roles_request_id_token_themselves_in_pr_workflows():
    offenders = []
    for p in workflows():
        wf = load(p)
        if not triggers(wf) & PR_EVENTS:
            continue
        for job_id, job, _ in aws_steps(wf):
            if not grants_id_token(job.get('permissions')):
                offenders.append(f'{p.name}:{job_id}')
    assert offenders == []


def test_default_branch_context_events_never_assume_aws_roles():
    offenders = [
        p.name for p in workflows()
        if triggers(load(p)) & DEFAULT_BRANCH_CONTEXT_EVENTS and any(aws_steps(load(p)))
    ]
    assert offenders == []
