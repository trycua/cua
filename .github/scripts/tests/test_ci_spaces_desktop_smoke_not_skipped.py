"""The desktop install smoke must test something, and no quick-gated job may be silently skipped.

`quick` only runs on pull_request. GitHub skips every job behind a skipped job
(through the whole needs chain) unless its own `if:` calls a status function, so
a manual dispatch used to go green with the Windows and Linux jobs skipped.
"""

import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = sorted((ROOT / ".github/workflows").glob("*.yml"))
STATUS = re.compile(r"\b(always|failure|cancelled|success)\(\)")
SMOKE = ROOT / ".github/workflows/ci-cua-spaces-desktop-install-smoke.yml"


def _jobs(path: Path) -> dict:
    return yaml.safe_load(path.read_text()).get("jobs") or {}


def _needs(job: dict) -> list[str]:
    needs = job.get("needs") or []
    return [needs] if isinstance(needs, str) else list(needs)


@pytest.mark.parametrize("path", WORKFLOWS, ids=lambda p: p.name)
def test_no_job_is_skipped_just_because_quick_is_skipped(path: Path) -> None:
    jobs = _jobs(path)
    if "if" not in (jobs.get("quick") or {}):
        pytest.skip("quick is not conditional")

    def behind_quick(job: str, seen: set[str]) -> bool:
        return any(
            dep == "quick" or (dep in jobs and dep not in seen and behind_quick(dep, seen | {dep}))
            for dep in _needs(jobs[job])
        )

    missing = [
        job
        for job, spec in jobs.items()
        if job != "quick" and behind_quick(job, set()) and not STATUS.search(str(spec.get("if", "")))
    ]
    assert not missing, f"{path.name}: skipped whenever quick is skipped: {missing}"


def test_smoke_fails_when_its_test_jobs_are_skipped() -> None:
    jobs = _jobs(SMOKE)
    verify = jobs["verify"]
    assert set(_needs(verify)) >= {"windows", "linux"}
    assert "always()" in str(verify["if"])
    script = "\n".join(str(s.get("run", "")) for s in verify["steps"])
    assert "exit 1" in script
    assert "needs.windows.result" in str(verify["steps"])
    assert "needs.linux.result" in str(verify["steps"])


@pytest.mark.parametrize("path", WORKFLOWS, ids=lambda p: p.name)
def test_no_blanket_skipped_needs_check(path: Path) -> None:
    """Depend on the result of the specific job needed; a blanket check also
    skipped jobs whose upstream job legitimately ran (observed on the smoke)."""
    text = path.read_text()
    assert not re.search(r"contains\(\s*needs\.\*\.result\s*,\s*'skipped'\s*\)", text), (
        f"{path.name}: use needs.<job>.result == 'success' instead of contains(needs.*.result, 'skipped')"
    )
