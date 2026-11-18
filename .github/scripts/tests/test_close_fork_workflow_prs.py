import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / ".github/scripts"))
import close_fork_workflow_prs as cf  # noqa: E402

REPO = "trycua/cua"
WORKFLOW = ROOT / ".github/workflows/close-fork-workflow-prs.yml"


def pull(number=5, head_repo="evil/cua", association="NONE", labels=(), created="2026-11-16T00:00:00Z"):
    return {
        "number": number,
        "created_at": created,
        "user": {"login": "x"},
        "author_association": association,
        "labels": [{"name": n} for n in labels],
        "head": {"repo": {"full_name": head_repo} if head_repo else None},
    }


class FakeClient:
    def __init__(self, pulls, files, comments=()):
        self.pulls, self.files, self.comments, self.calls = pulls, files, list(comments), []

    def paged(self, path):
        if path.endswith("/files"):
            return self.files[int(path.split("/")[-2])]
        if path.endswith("/comments"):
            return self.comments
        return self.pulls

    def send(self, method, path, payload=None):
        self.calls.append((method, path.split("/", 3)[3], payload))


def test_fork_changing_workflow_is_commented_and_closed():
    client = FakeClient([pull()], {5: [{"filename": ".github/workflows/ci.yml"}]})
    assert cf.run(client, REPO, dry_run=False) == [5]
    assert [(m, p) for m, p, _ in client.calls] == [("POST", "issues/5/comments"), ("PATCH", "pulls/5")]
    assert client.calls[1][2] == {"state": "closed"}


def test_composite_action_and_renames_count():
    assert cf.touches_ci([{"filename": ".github/actions/x/action.yml"}])
    assert cf.touches_ci([{"filename": "ci.yml", "previous_filename": ".github/workflows/ci.yml"}])
    assert not cf.touches_ci([{"filename": "docs/.github/workflows/x.yml"}, {"filename": ".github/CODEOWNERS"}])


def test_deleted_fork_counts_as_fork():
    assert cf.is_fork(pull(head_repo=None), REPO)
    assert not cf.is_fork(pull(head_repo=REPO), REPO)


def test_trusted_authors_and_override_label_are_left_alone():
    files = {5: [{"filename": ".github/workflows/ci.yml"}]}
    for p in (pull(association="MEMBER"), pull(association="COLLABORATOR"), pull(labels=[cf.OVERRIDE_LABEL]), pull(head_repo=REPO)):
        client = FakeClient([p], files)
        assert cf.run(client, REPO, dry_run=False) == []
        assert client.calls == []


def test_fork_without_ci_changes_is_left_alone():
    client = FakeClient([pull()], {5: [{"filename": "README.md"}]})
    assert cf.run(client, REPO, dry_run=False) == []


def test_dry_run_and_existing_comment():
    files = {5: [{"filename": ".github/workflows/ci.yml"}]}
    client = FakeClient([pull()], files)
    assert cf.run(client, REPO, dry_run=True) == [5] and client.calls == []
    client = FakeClient([pull()], files, comments=[{"body": cf.MARKER}])
    cf.run(client, REPO, dry_run=False)
    assert [(m, p) for m, p, _ in client.calls] == [("PATCH", "pulls/5")]


def test_workflow_never_runs_pull_request_code():
    wf = yaml.safe_load(WORKFLOW.read_text())
    assert set(wf[True]) == {"schedule", "workflow_dispatch"}
    assert wf["permissions"] == {"contents": "read", "pull-requests": "write"}
    text = WORKFLOW.read_text()
    assert "secrets." not in text and "id-token" not in text
    for step in wf["jobs"]["close"]["steps"]:
        if str(step.get("uses", "")).startswith("actions/checkout"):
            assert "ref" not in step["with"] and "repository" not in step["with"]


def test_pull_requests_opened_before_the_cutoff_are_left_alone():
    client = FakeClient([pull(created="2026-11-14T23:59:59Z")], {5: [{"filename": ".github/workflows/ci.yml"}]})
    assert cf.run(client, REPO, dry_run=False) == []
