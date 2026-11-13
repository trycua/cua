"""A failed sccache download never fails the job that uses the Rust cache.

The action promises that cargo compiles without sccache when it cannot start.
On 9 Nov 2026 GitHub's release CDN answered the sccache download with 504s,
and every Rust job of the pull requests that ran then failed at "Install
sccache" (and again at its post step) before compiling.
"""

from __future__ import annotations

import pathlib

import yaml

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
ACTION = REPO_ROOT / ".github/actions/rust-cache/action.yml"


def steps() -> list[dict]:
    return yaml.safe_load(ACTION.read_text())["runs"]["steps"]


def test_the_sccache_install_may_fail():
    install = [s for s in steps() if str(s.get("uses", "")).startswith("mozilla-actions/sccache-action@")]
    assert len(install) == 1
    assert install[0].get("continue-on-error") is True


def test_cargo_runs_without_sccache_when_it_did_not_install():
    route = next(s for s in steps() if s.get("name") == "Route rustc through sccache")
    assert "if" not in route
    assert "if sccache --start-server" in route["run"]
    assert "compiling without it" in route["run"]


def test_a_job_that_saves_after_its_steps_saves_with_always():
    """A rust-cache step with save-always "true" leaves the save to a later rust-cache-save step. That step
    must run with always() and read the same step's outputs, or the warm compile is lost whenever a test
    fails (CI: cua SDK's Windows rust leg compiled cold on every run for this)."""
    for path in sorted((REPO_ROOT / ".github/workflows").glob("*.yml")):
        for name, job in (yaml.safe_load(path.read_text()).get("jobs") or {}).items():
            steps = job.get("steps") or []
            caches = [s for s in steps if str(s.get("uses", "")).endswith("/actions/rust-cache")
                      and str((s.get("with") or {}).get("save-always", "")) == "true"]
            for cache in caches:
                assert cache.get("id"), f"{path.name}: {name}"
                saves = [s for s in steps if str(s.get("uses", "")).endswith("/actions/rust-cache-save")]
                assert saves, f"{path.name}: {name} never saves"
                save = saves[-1]
                assert steps.index(save) == len(steps) - 1, f"{path.name}: {name}: the save is not the last step"
                assert str(save.get("if", "")).startswith("${{ always()"), f"{path.name}: {name}"
                for key in ("key", "dir", "exact-hit"):
                    assert save["with"][key] == f"${{{{ steps.{cache['id']}.outputs.{key} }}}}", f"{path.name}: {name}: {key}"


def test_cua_sdk_windows_rust_leg_keeps_its_compile():
    steps = yaml.safe_load((REPO_ROOT / ".github/workflows/ci-cua-sdk.yml").read_text())["jobs"]["rust"]["steps"]
    cache = next(s for s in steps if str(s.get("uses", "")).endswith("/actions/rust-cache"))
    assert cache["with"]["save-always"] == "true"
    # A job that hits its own timeout is cancelled and skips the save, so the long test step stops first
    # (a step timeout fails only that step).
    test = next(s for s in steps if s.get("name", "").startswith("Test (embedded"))
    assert test["timeout-minutes"] <= 45
