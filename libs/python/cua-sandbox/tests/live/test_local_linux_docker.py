"""Opt-in pytest wrapper for the local Linux Docker live script."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).parents[2] / "scripts" / "live_local_linux_docker.py"
ENABLED = os.environ.get("CUA_TEST_LOCAL_LINUX_DOCKER", "").lower() in {
    "1",
    "true",
    "yes",
}

pytestmark = pytest.mark.skipif(
    not ENABLED,
    reason="CUA_TEST_LOCAL_LINUX_DOCKER is not enabled",
)


def test_local_linux_docker_live_script() -> None:
    result = subprocess.run(
        [str(SCRIPT)],
        text=True,
        check=False,
    )
    assert result.returncode == 0
