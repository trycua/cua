"""Fixtures for the binding smoke tests.

`cua-test-fixtures` (built with `libs/cua/scripts/build-test-fixtures.sh`)
serves a MockServer spacesd with
a scripted media socket and a fake Fleet API on loopback. Nothing here
touches host apps; the fixture process exits when its stdin closes.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

import pytest

CUA_ROOT = Path(__file__).resolve().parents[2]

# Never the user's real ~/.cua: a private CUA_HOME for the whole session,
# set before the binding loads, and CUA_TEST=1 so the SDK refuses any write
# to the real home (Spaces registry, sandbox state, tokens) instead of
# leaking it into their Spaces app. Tests that need their own home still
# pass `state_dir` / `spaces_home` or monkeypatch CUA_HOME.
os.environ["CUA_TEST"] = "1"
os.environ["CUA_HOME"] = tempfile.mkdtemp(prefix="cua-py-test-")


def _binary(env: str, name: str) -> Path | None:
    explicit = os.environ.get(env)
    if explicit:
        return Path(explicit)
    exe = name + (".exe" if os.name == "nt" else "")
    for profile in ("debug", "release"):
        candidate = CUA_ROOT / "target" / profile / exe
        if candidate.exists():
            return candidate
    return None


@pytest.fixture(scope="session")
def fixtures():
    binary = _binary("CUA_TEST_FIXTURES", "cua-test-fixtures")
    if binary is None:
        pytest.skip("cua-test-fixtures is not built")
    proc = subprocess.Popen(
        [str(binary)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        line = proc.stdout.readline()
        if not line:
            # A stale fixture (built from another commit) refuses to serve and
            # says so on stderr.
            raise RuntimeError(
                "cua-test-fixtures exited before printing endpoints; if it is stale, "
                "rebuild it with libs/cua/scripts/build-test-fixtures.sh"
            )
        yield json.loads(line)
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)


@pytest.fixture(scope="session")
def cua_binary():
    binary = _binary("CUA_CLI", "cua")
    if binary is None:
        pytest.skip("the cua CLI is not built")
    return binary


@pytest.fixture(scope="session")
def spaces_cli_binary():
    """The Cua Spaces build of `cua` (source-available, FSL-1.1-MIT), whose
    daemon serves teleport, the Cua Volume and persistent agents."""
    binary = _binary("CUA_SPACES_CLI", "cua-spaces-cli")
    if binary is None:
        pytest.skip("the Cua Spaces CLI (cua-spaces-cli) is not built")
    return binary
