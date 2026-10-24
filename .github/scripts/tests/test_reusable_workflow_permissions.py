"""A called workflow's jobs may not ask for more permission than the caller grants.

GitHub checks this statically when it starts a run, including for jobs whose
`if:` is false. A violation does not fail a job. The whole run ends as
`startup_failure` with no jobs, no log and no annotation reachable from the
API. `nightly-cua-driver` sat in that state from 20 to 24 Oct 2026 because
`cd-rust-cua-driver.yml` gained a `staging-gate` job that asks for
`statuses: read` (#5539) while the nightly caller grants no `statuses` scope.
actionlint does not check this, so this test does.

Only callers that declare `permissions` (at workflow or job level) are checked.
A caller with no declaration gets the repository default, which this test
cannot see.
"""

from __future__ import annotations

from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github/workflows"
LEVEL = {"none": 0, "read": 1, "write": 2}
# Every scope GITHUB_TOKEN can hold; `read-all` and `write-all` expand to these.
# `metadata: read` is always available, so it is left out.
TOKEN_SCOPES = (
    "actions",
    "attestations",
    "checks",
    "contents",
    "deployments",
    "discussions",
    "id-token",
    "issues",
    "models",
    "packages",
    "pages",
    "pull-requests",
    "repository-projects",
    "security-events",
    "statuses",
)


def load(path: Path) -> dict:
    return yaml.safe_load(path.read_text()) or {}


def scopes(perms) -> dict[str, int] | None:
    """Normalise a `permissions:` value to {scope: level}. None means not declared."""
    if perms is None:
        return None
    if perms == "read-all":
        return {scope: LEVEL["read"] for scope in TOKEN_SCOPES}
    if perms == "write-all":
        return {scope: LEVEL["write"] for scope in TOKEN_SCOPES}
    return {scope: LEVEL[str(level)] for scope, level in perms.items() if scope != "metadata"}


def level_of(granted: dict[str, int], scope: str) -> int:
    return granted.get(scope, 0)


def name_of(level: int) -> str:
    return next(name for name, value in LEVEL.items() if value == level)


def effective(workflow: dict, job: dict) -> dict[str, int] | None:
    declared = scopes(job.get("permissions"))
    return declared if declared is not None else scopes(workflow.get("permissions"))


def violations(caller_name: str, granted: dict[str, int], callee_path: Path, trail: tuple[str, ...]):
    """Yield one message per scope a called job asks for above what its caller grants."""
    callee = load(callee_path)
    for job_id, job in (callee.get("jobs") or {}).items():
        wanted = effective(callee, job)
        here = trail + (f"{callee_path.name}:{job_id}",)
        if wanted is not None:
            for scope, level in wanted.items():
                if level > level_of(granted, scope):
                    yield (
                        f"{caller_name} -> {' -> '.join(here)} asks for {scope}: {name_of(level)} "
                        f"but the caller grants {scope}: {name_of(level_of(granted, scope))}"
                    )
        uses = job.get("uses", "")
        if uses.startswith("./.github/workflows/"):
            nested = ROOT / uses.removeprefix("./")
            # A nested call is bounded by what this job was granted. A job that
            # declares nothing inherits the whole chain's ceiling.
            yield from violations(caller_name, wanted if wanted is not None else granted, nested, here)


def all_violations() -> list[str]:
    found: list[str] = []
    for path in sorted([*WORKFLOWS.glob("*.yml"), *WORKFLOWS.glob("*.yaml")]):
        workflow = load(path)
        for job_id, job in (workflow.get("jobs") or {}).items():
            uses = job.get("uses", "")
            if not uses.startswith("./.github/workflows/"):
                continue
            granted = effective(workflow, job)
            if granted is None:
                continue
            found.extend(violations(path.name, granted, ROOT / uses.removeprefix("./"), (f"{job_id}",)))
    return found


def test_called_workflows_stay_inside_the_callers_permissions():
    found = all_violations()
    assert not found, "startup_failure: a called job asks for more than its caller grants\n" + "\n".join(found)


def test_checker_catches_the_nightly_cua_driver_failure(tmp_path):
    callee = tmp_path / "callee.yml"
    callee.write_text(
        "on: workflow_call\n"
        "jobs:\n"
        "  gate:\n"
        "    permissions:\n"
        "      contents: read\n"
        "      statuses: read\n"
        "    runs-on: ubuntu-latest\n"
        "    steps: []\n"
    )
    found = list(violations("nightly.yml", {"contents": 2, "issues": 2}, callee, ("build",)))
    assert len(found) == 1 and "statuses: read" in found[0]
    assert not list(violations("nightly.yml", {"contents": 2, "statuses": 1}, callee, ("build",)))


def test_read_all_and_write_all_expand_to_explicit_scopes():
    assert scopes("read-all") == {scope: 1 for scope in TOKEN_SCOPES}
    assert scopes("write-all") == {scope: 2 for scope in TOKEN_SCOPES}
    assert scopes({"statuses": "read", "metadata": "read"}) == {"statuses": 1}
    assert scopes({}) == {}
    assert scopes(None) is None
    # A caller that lists every scope explicitly is equivalent to read-all.
    callee = {"permissions": "read-all", "jobs": {}}
    assert effective(callee, {}) == {scope: 1 for scope in TOKEN_SCOPES}


def test_yaml_extension_callers_are_scanned(tmp_path, monkeypatch):
    workflows = tmp_path / ".github/workflows"
    workflows.mkdir(parents=True)
    (workflows / "callee.yml").write_text(
        "on: workflow_call\n"
        "jobs:\n"
        "  gate:\n"
        "    permissions: {statuses: read}\n"
        "    runs-on: ubuntu-latest\n"
        "    steps: []\n"
    )
    (workflows / "caller.yaml").write_text(
        "on: push\n"
        "permissions: {contents: read}\n"
        "jobs:\n"
        "  call:\n"
        "    uses: ./.github/workflows/callee.yml\n"
    )
    monkeypatch.setattr(f"{__name__}.ROOT", tmp_path)
    monkeypatch.setattr(f"{__name__}.WORKFLOWS", workflows)
    found = all_violations()
    assert len(found) == 1 and found[0].startswith("caller.yaml -> call -> callee.yml:gate")
