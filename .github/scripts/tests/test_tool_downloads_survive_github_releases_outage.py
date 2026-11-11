"""Pull request lanes do not fail when one download host answers 5xx.

On 8 to 10 Nov 2026 the pull request reruns of `E2E: cua SDK`, the Swift
sample lanes and `CD: Image linux` were almost all one class of failure: a
pinned tool or artifact fetched from a single host with no second source.
GitHub release downloads timed out (actionlint, uv), artifacts were listed
before they were served, and proxy.golang.org 404ed a new Go toolchain. A
rerun passed every time, so each one cost a full workflow of runner time and
a gate round trip.
"""

from __future__ import annotations

import pathlib
import re
import subprocess

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github/workflows"
ACTIONS = ROOT / ".github/actions"
SETUP_UV = "astral-sh/setup-uv@11f9893b081a58869d3b5fccaea48c9e9e46f990"  # v8.3.2


def every_uses() -> list[tuple[str, str]]:
    out = []
    for path in sorted(WORKFLOWS.glob("*.yml")) + sorted(ACTIONS.glob("*/action.yml")):
        for line in path.read_text().splitlines():
            m = re.match(r"\s*(?:-\s+)?uses:\s*(\S+)", line)
            if m:
                out.append((str(path.relative_to(ROOT)), m.group(1)))
    return out


def test_setup_uv_downloads_from_the_astral_mirror():
    # v8 and later fetch uv from releases.astral.sh and fall back to GitHub
    # Releases; v6 fetched only from GitHub Releases.
    uses = [(f, u) for f, u in every_uses() if u.startswith("astral-sh/setup-uv@")]
    assert uses
    assert {u for _, u in uses} == {SETUP_UV}, [f for f, u in uses if u != SETUP_UV]
    for path in sorted(WORKFLOWS.glob("*.yml")):
        assert "download-from-astral-mirror: false" not in path.read_text(), path.name


def actionlint_steps() -> list[dict]:
    return yaml.safe_load((ACTIONS / "install-actionlint/action.yml").read_text())["runs"]["steps"]


def test_actionlint_is_cached_and_has_a_second_download_path():
    steps = actionlint_steps()
    cache = steps[0]
    assert cache["uses"].startswith("actions/cache@")
    assert cache["with"]["key"] == "actionlint-${{ inputs.version }}-${{ runner.os }}-${{ runner.arch }}"
    run = steps[1]["run"]
    assert 'if [ -x "$dir/actionlint" ]' in run  # a cache hit makes no network call
    assert "gh release download" in run and "--repo rhysd/actionlint" in run


def run_actionlint_install(tmp_path: pathlib.Path, curl_ok: bool, cached: bool) -> tuple[subprocess.CompletedProcess, str]:
    """Run the install step with stub curl, gh and tar; returns the result and what was called."""
    stubs = tmp_path / "bin"
    stubs.mkdir()
    calls = tmp_path / "calls"
    for name, body in {
        "curl": f'echo curl >> "{calls}"; exit {0 if curl_ok else 22}',
        "gh": f'echo gh >> "{calls}"',
        "tar": f'echo tar >> "{calls}"; printf "#!/bin/sh\\necho 1.7.12\\n" > "$4/actionlint"; chmod +x "$4/actionlint"',
    }.items():
        (stubs / name).write_text(f"#!/bin/sh\n{body}\n")
        (stubs / name).chmod(0o755)
    temp = tmp_path / "temp"
    (temp / "actionlint-bin").mkdir(parents=True)
    if cached:
        (temp / "actionlint-bin/actionlint").write_text("#!/bin/sh\necho 1.7.12\n")
        (temp / "actionlint-bin/actionlint").chmod(0o755)
    env = {"PATH": f"{stubs}:/usr/bin:/bin", "RUNNER_TEMP": str(temp), "GITHUB_PATH": str(tmp_path / "path"),
           "VERSION": "1.7.12", "GH_TOKEN": "t"}
    done = subprocess.run(["bash", "-e", "-c", actionlint_steps()[1]["run"]], env=env, capture_output=True, text=True, timeout=20)
    return done, calls.read_text() if calls.exists() else ""


def test_actionlint_install_falls_back_to_gh_when_the_release_download_fails(tmp_path):
    done, calls = run_actionlint_install(tmp_path, curl_ok=False, cached=False)
    assert done.returncode == 0, done.stderr
    assert calls.split() == ["curl", "gh", "tar"]
    assert "falling back to gh release download" in done.stdout


def test_actionlint_install_makes_no_network_call_on_a_cache_hit(tmp_path):
    done, calls = run_actionlint_install(tmp_path, curl_ok=False, cached=True)
    assert done.returncode == 0, done.stderr
    assert calls == ""


def test_lanes_retry_an_artifact_another_job_just_uploaded():
    for name in ("e2e-cua-sdk.yml", "cd-image-linux.yml"):
        text = (WORKFLOWS / name).read_text()
        assert "actions/download-artifact@" not in text, name
        assert "uses: ./.github/actions/download-fresh-artifact" in text, name
    steps = yaml.safe_load((ACTIONS / "download-fresh-artifact/action.yml").read_text())["runs"]["steps"]
    first, wait, second = steps
    assert first["continue-on-error"] is True and first["id"] == "first"
    assert wait["if"] == second["if"] == "steps.first.outcome == 'failure'"
    assert "sleep 30" in wait["run"]
    assert first["uses"] == second["uses"] and first["with"] == second["with"]


def test_release_tools_never_download_a_go_toolchain():
    script = (ROOT / "scripts/images/install-release-tools.sh").read_text()
    pick = script.index("cached_go=")
    first_install = script.index("|| go install ")
    assert "GOTOOLCHAIN=local" in script[pick:first_install]
    # The pick comes before the first go command.
    assert pick < script.index("go env GOPATH") < first_install
