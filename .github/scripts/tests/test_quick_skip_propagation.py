"""A PR-only `quick` job must not skip the jobs behind it on other events.

GitHub skips a job whose `needs` chain contains a skipped job unless its `if:`
calls a status function. When `quick` only runs on pull_request, a job without
one is skipped on schedule, push and workflow_call runs.
"""

import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = (
    "e2e-cua-driver-release-gate.yml",
    "e2e-rust-linux.yml",
)
STATUS = re.compile(r"\b(always|failure|cancelled|success)\(\)")


def _needs(job: dict) -> list[str]:
    needs = job.get("needs") or []
    return [needs] if isinstance(needs, str) else list(needs)


@pytest.mark.parametrize("name", WORKFLOWS)
def test_jobs_behind_pr_only_quick_survive_its_skip(name: str) -> None:
    jobs = yaml.safe_load((ROOT / ".github/workflows" / name).read_text())["jobs"]
    assert "pull_request" in jobs["quick"]["if"]

    def behind_quick(job: str, seen: set[str]) -> bool:
        for dep in _needs(jobs[job]):
            if dep == "quick" or (dep not in seen and behind_quick(dep, seen | {dep})):
                return True
        return False

    missing = [
        job
        for job, spec in jobs.items()
        if job != "quick" and behind_quick(job, set()) and not STATUS.search(str(spec.get("if", "")))
    ]
    assert not missing, f"{name}: jobs skipped whenever quick is skipped: {missing}"
