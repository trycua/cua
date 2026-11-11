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
