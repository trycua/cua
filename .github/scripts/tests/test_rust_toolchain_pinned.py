"""Rust jobs that run rustfmt or clippy must use a pinned toolchain.

`dtolnay/rust-toolchain@stable` installs whatever stable is today. When Rust
1.100.0 shipped, its rustfmt formatted `raw_client!` invocations differently,
so `cargo fmt --check` failed on every PR (and on main) with no code change.
Pin the compiler: `dtolnay/rust-toolchain@<x.y.z>`, or no install step at all
so rustup reads the workspace's `rust-toolchain.toml`.
"""

import re
import tomllib
from pathlib import Path

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = sorted((ROOT / ".github/workflows").glob("*.yml"))

# `dtolnay/rust-toolchain` SHAs that are the moving `stable` branch.
FLOATING_SHAS = {"6bed0761d98439e5a578e2877258200ad565ba87"}
FLOATING_CHANNELS = {"stable", "beta", "nightly", "master"}
LINTS = re.compile(r"\bcargo\s+(\+\S+\s+)?(fmt|clippy)\b|\brustfmt\b|\bcargo-clippy\b")
FLOATING_RUN = re.compile(r"rustup\s+(default|update|toolchain\s+install)\s+(stable|beta|nightly)\b")
PINNED = re.compile(r"^(\d+\.\d+(\.\d+)?|(nightly|beta)-\d{4}-\d{2}-\d{2})$")


def _jobs():
    for path in WORKFLOWS:
        for name, job in (yaml.safe_load(path.read_text()).get("jobs") or {}).items():
            if isinstance(job, dict) and job.get("steps"):
                yield path.name, name, job["steps"]


def _floating_installs(steps):
    found = []
    for step in steps:
        uses = step.get("uses") or ""
        if uses.startswith("dtolnay/rust-toolchain@"):
            ref = uses.split("@", 1)[1]
            with_ = step.get("with") or {}
            channel = str(with_.get("toolchain") or ref)
            if ref in FLOATING_SHAS or channel in FLOATING_CHANNELS:
                found.append(uses + (f" (toolchain: {channel})" if channel != ref else ""))
        if FLOATING_RUN.search(step.get("run") or ""):
            found.append(step["run"].strip().splitlines()[0])
    return found


def _lints(steps):
    return any(LINTS.search(step.get("run") or "") for step in steps)


def test_no_rust_lint_job_uses_a_floating_toolchain():
    offenders = [
        f"{wf}: job {job}: {install}"
        for wf, job, steps in _jobs()
        if _lints(steps)
        for install in _floating_installs(steps)
    ]
    assert not offenders, (
        "These jobs run rustfmt/clippy on a floating Rust toolchain, so a new Rust release can "
        "break CI with no code change. Use dtolnay/rust-toolchain@<x.y.z> matching the workspace's "
        "rust-toolchain.toml:\n" + "\n".join(offenders)
    )


def test_pinned_install_steps_match_the_workspace_toolchain():
    pinned = tomllib.loads((ROOT / "libs/cua/rust-toolchain.toml").read_text())["toolchain"]["channel"]
    mismatched = []
    for wf, job, steps in _jobs():
        if not _lints(steps):
            continue
        for step in steps:
            uses = step.get("uses") or ""
            if uses.startswith("dtolnay/rust-toolchain@") and not (step.get("with") or {}).get("toolchain"):
                ref = uses.split("@", 1)[1]
                if PINNED.match(ref) and ref != pinned and not ref.startswith("nightly"):
                    mismatched.append(f"{wf}: job {job}: {uses}")
    assert not mismatched, f"Pin to {pinned} (libs/cua/rust-toolchain.toml):\n" + "\n".join(mismatched)


@pytest.mark.parametrize("path", sorted(ROOT.glob("**/rust-toolchain.toml"), key=str))
def test_rust_toolchain_files_pin_an_exact_channel(path):
    if any(part in {"node_modules", "target", ".worktrees"} for part in path.parts):
        pytest.skip("not tracked source")
    channel = tomllib.loads(path.read_text())["toolchain"]["channel"]
    assert PINNED.match(channel), f"{path.relative_to(ROOT)}: channel {channel!r} floats"


def test_cua_spacesd_workspace_pins_the_same_toolchain_as_libs_cua():
    cua = tomllib.loads((ROOT / "libs/cua/rust-toolchain.toml").read_text())["toolchain"]
    spacesd = tomllib.loads((ROOT / "libs/cua-spacesd/rust-toolchain.toml").read_text())["toolchain"]
    assert spacesd["channel"] == cua["channel"]
    assert {"rustfmt", "clippy"} <= set(spacesd["components"])
