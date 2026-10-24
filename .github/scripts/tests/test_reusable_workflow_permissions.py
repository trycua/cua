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
# `metadata: read` is always available.
IGNORED_SCOPES = {"metadata"}


def load(path: Path) -> dict:
    return yaml.safe_load(path.read_text()) or {}


def scopes(perms) -> dict[str, int] | None:
    """Normalise a `permissions:` value to {scope: level}. None means not declared."""
    if perms is None:
        return None
    if perms == "read-all":
        return {"*": 1}
    if perms == "write-all":
        return {"*": 2}
    return {scope: LEVEL[str(level)] for scope, level in perms.items() if scope not in IGNORED_SCOPES}


def level_of(granted: dict[str, int], scope: str) -> int:
    return granted.get(scope, granted.get("*", 0))


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
                        f"{caller_name} -> {' -> '.join(here)} asks for {scope}: "
                        f"{[k for k, v in LEVEL.items() if v == level][0]} "
                        f"but the caller grants {scope}: "
                        f"{[k for k, v in LEVEL.items() if v == level_of(granted, scope)][0]}"
                    )
        uses = job.get("uses", "")
        if uses.startswith("./.github/workflows/"):
            nested = ROOT / uses.removeprefix("./")
            # A nested call is bounded by what this job was granted. A job that
            # declares nothing inherits the whole chain's ceiling.
            yield from violations(caller_name, wanted if wanted is not None else granted, nested, here)


def all_violations() -> list[str]:
    found: list[str] = []
    for path in sorted(WORKFLOWS.glob("*.yml")):
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
