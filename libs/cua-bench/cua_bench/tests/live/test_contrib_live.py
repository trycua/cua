"""LIVE: ``cb run --on <provider>`` on a contrib sandbox provider (opt-in,
creates a real sandbox on the provider, which bills the key's account).

Run only on purpose, with a cua binding built with ``--features contrib``
and the provider's key::

    CUA_E2E_CONTRIB_LIVE=1 CUA_E2E_CONTRIB_PROVIDER=e2b E2B_API_KEY=... \\
        .venv/bin/python -m pytest -q -s cua_bench/tests/live/test_contrib_live.py

It runs the bundled ``hello_file_env`` task through the real CLI (the same
code path as ``--on local``), capped at ``--cpu 2 --memory 4G``, and checks
the reward. The sandbox is ephemeral: the SDK deletes it when the variant
ends, and the provider's lifetime backstop covers a killed run. Keys:
``E2B_API_KEY``, ``DAYTONA_API_KEY``, or ``MODAL_TOKEN_ID`` +
``MODAL_TOKEN_SECRET`` (Modal also needs ``cua-modal-helper``).
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

PROVIDER = os.environ.get("CUA_E2E_CONTRIB_PROVIDER", "e2b")
KEYS = {
    "e2b": ["E2B_API_KEY"],
    "daytona": ["DAYTONA_API_KEY"],
    "modal": ["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"],
}

pytestmark = [
    pytest.mark.skipif(
        os.environ.get("CUA_E2E_CONTRIB_LIVE") != "1",
        reason="opt-in: CUA_E2E_CONTRIB_LIVE=1 creates a real sandbox on a contrib provider",
    ),
    pytest.mark.skipif(
        not all(os.environ.get(k) for k in KEYS.get(PROVIDER, ["?"])),
        reason=f"{PROVIDER}: set {' and '.join(KEYS.get(PROVIDER, ['a known provider']))}",
    ),
]

HELLO = Path(__file__).resolve().parents[3] / "example_tasks" / "hello_file_env"


def test_hello_task_on_a_contrib_provider(tmp_path):
    out = tmp_path / "out"
    env = {
        **os.environ,
        "XDG_DATA_HOME": str(tmp_path / "data"),
        "XDG_STATE_HOME": str(tmp_path / "state"),
        "CUA_BENCH_NO_BANNER": "1",
        "CUA_TELEMETRY_ENABLED": "false",
        "PYTHONUNBUFFERED": "1",
    }
    proc = subprocess.run(
        [
            sys.executable,
            "-m",
            "cua_bench.cli.main",
            "run",
            str(HELLO),
            "--on",
            PROVIDER,
            "--variant-id",
            "1",
            "--cpu",
            "2",
            "--memory",
            "4G",
            "--output-dir",
            str(out),
        ],
        env=env,
        capture_output=True,
        text=True,
        timeout=3600,
    )
    print(proc.stdout[-4000:], proc.stderr[-2000:])
    assert proc.returncode == 0, proc.stdout[-2000:]
    result = json.loads((out / "hello_file_env_v1" / "result.json").read_text())
    assert result["status"] == "completed" and result["reward"] == 1.0
    assert result["on"] == PROVIDER
