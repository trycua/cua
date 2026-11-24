"""LIVE: ``cb run`` on a local sandbox (opt-in, starts a real container).

Run only on purpose (needs docker, the cua SDK's native library and the
canonical Linux image or network access to pull it)::

    CUA_BENCH_E2E_LOCAL=1 .venv/bin/python -m pytest -q -s cua_bench/tests/live/test_local_live.py

It runs the bundled ``hello_file_env`` task through the real CLI in a
subprocess, capped at ``--cpu 2 --memory 2G``, and checks the reward and the
recorded runtime/image facts. The sandbox is ephemeral (the SDK removes it
when the variant ends); the test also asserts no local sandbox is left.

``CUA_BENCH_E2E_LOCAL_VM=1`` adds the same task as a local QEMU VM
(``--runtime vm``, at most one VM, 4 GiB).
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.skipif(
    os.environ.get("CUA_BENCH_E2E_LOCAL") != "1",
    reason="opt-in: CUA_BENCH_E2E_LOCAL=1 starts a real local sandbox",
)

HELLO = Path(__file__).resolve().parents[3] / "example_tasks" / "hello_file_env"


def _cb(args: list[str], tmp: Path, timeout: int = 1200) -> subprocess.CompletedProcess:
    env = {
        **os.environ,
        "XDG_DATA_HOME": str(tmp / "data"),
        "XDG_STATE_HOME": str(tmp / "state"),
        "CUA_BENCH_NO_BANNER": "1",
        "CUA_TELEMETRY_ENABLED": "false",
        "PYTHONUNBUFFERED": "1",
    }
    return subprocess.run(
        [sys.executable, "-m", "cua_bench.cli.main", *args],
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _local_sandboxes() -> set[str]:
    import asyncio

    from cua_sandbox import Sandbox

    return {sb.name for sb in asyncio.run(Sandbox.list(local=True))}


def _run_and_check(tmp_path: Path, extra: list[str], runtime: str) -> dict:
    before = _local_sandboxes()
    out = tmp_path / f"out-{runtime}"
    proc = _cb(
        [
            "run",
            str(HELLO),
            "--variant-id",
            "1",
            "--cpu",
            "2",
            "--memory",
            "2G",
            "--output-dir",
            str(out),
            *extra,
        ],
        tmp_path,
    )
    print(proc.stdout[-4000:], proc.stderr[-2000:])
    assert proc.returncode == 0, proc.stdout[-2000:]
    result = json.loads((out / "hello_file_env_v1" / "result.json").read_text())
    assert result["status"] == "completed" and result["reward"] == 1.0
    assert result["on"] == "local" and result["runtime"] == runtime
    assert result["image_ref"]
    assert (out / "hello_file_env_v1" / "task_1_trace").is_dir()
    assert _local_sandboxes() <= before, "the ephemeral sandbox was not removed"
    return result


def test_local_container_end_to_end(tmp_path):
    result = _run_and_check(tmp_path, [], "container")
    assert result["image_variant"] == "rootfs"
    print("image_digest:", result["image_digest"], "arch:", result["arch"])


@pytest.mark.skipif(
    os.environ.get("CUA_BENCH_E2E_LOCAL_VM") != "1", reason="opt-in: CUA_BENCH_E2E_LOCAL_VM=1"
)
def test_local_vm_end_to_end(tmp_path):
    result = _run_and_check(tmp_path, ["--runtime", "vm", "--memory", "4G"], "vm")
    assert result["image_variant"] == "containerdisk"
