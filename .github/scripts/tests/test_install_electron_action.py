"""The Electron shell's CI jobs survive a failed Electron download.

Electron 44 downloads its binary from GitHub Releases on the first
`require("electron")`, with one fetch and no retry. On 8 and 9 Nov 2026 that
fetch got a 504 and the Electron shell's vitest suite and its parity flows
failed on "Electron failed to install correctly". The install-electron action
fetches it once, before the tests, from a cache or with retries.
"""

from __future__ import annotations

import pathlib
import subprocess

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[3]
ACTION = ROOT / ".github/actions/install-electron/action.yml"
WORKFLOW = ROOT / ".github/workflows/ci-cua-spaces-web.yml"


def steps() -> list[dict]:
    return yaml.safe_load(ACTION.read_text())["runs"]["steps"]


def run_install(tmp_path: pathlib.Path, failures: int) -> tuple[subprocess.CompletedProcess, list[str]]:
    """Run the install step with a stub node whose install.js fails `failures` times."""
    stubs = tmp_path / "bin"
    stubs.mkdir()
    calls = tmp_path / "calls"
    binary = tmp_path / "dist/electron"
    (stubs / "node").write_text(f"""#!/bin/sh
case "$1" in
  -p) echo /pkg/electron/install.js ;;
  -e) echo "check $electron_config_cache" >> "{calls}"; test -e "{binary}" && echo "{binary}" ;;
  *) echo install >> "{calls}"
     n=$(grep -c install "{calls}")
     [ "$n" -gt {failures} ] || exit 1
     mkdir -p "{binary.parent}"; touch "{binary}" ;;
esac
""")
    (stubs / "node").chmod(0o755)
    (stubs / "sleep").write_text(f'#!/bin/sh\necho "sleep $1" >> "{calls}"\n')
    (stubs / "sleep").chmod(0o755)
    step = steps()[2]
    env = {"PATH": f"{stubs}:/usr/bin:/bin", "electron_config_cache": "/runner/temp/electron-cache"}
    done = subprocess.run(["bash", "-e", "-c", step["run"]], env=env, capture_output=True, text=True, timeout=20)
    return done, calls.read_text().splitlines() if calls.exists() else []


def test_the_download_is_cached_per_version_os_and_arch():
    cache = steps()[1]
    assert cache["uses"].startswith("actions/cache@")
    assert cache["with"]["key"] == "electron-${{ steps.version.outputs.version }}-${{ runner.os }}-${{ runner.arch }}"
    # The installer reads its cache from the same directory the cache step restores.
    assert steps()[2]["env"]["electron_config_cache"] == cache["with"]["path"]


def test_a_failed_download_is_retried_until_it_succeeds(tmp_path):
    done, calls = run_install(tmp_path, failures=2)
    assert done.returncode == 0, done.stderr
    assert calls == ["install", "sleep 15", "install", "sleep 30", "install",
                     "check /runner/temp/electron-cache"]
    assert done.stdout.count("::warning::Electron download attempt") == 2


def test_the_retries_are_bounded(tmp_path):
    done, calls = run_install(tmp_path, failures=10)
    assert done.returncode == 1
    assert calls.count("install") == 4
    assert "::error::Electron download failed after 4 attempts" in done.stdout


def test_the_electron_jobs_install_it_before_their_tests():
    jobs = yaml.safe_load(WORKFLOW.read_text())["jobs"]
    for job, first_user in (("test", "Electron shell typecheck, tests (router contract) and build (channel check)"),
                            ("parity", "Parity flows and the bridge on the Electron shell (demo mode)")):
        names = [s.get("name") or s.get("uses") for s in jobs[job]["steps"]]
        install = names.index("./.github/actions/install-electron")
        assert install < names.index(first_user), job
        assert jobs[job]["steps"][install]["with"]["working-directory"] == "apps/cua-spaces-desktop"
