"""LIVE: ``cb run --on cloud`` on managed Fleet pools (opt-in, billed).

Run only on purpose::

    set -a; source ~/.env; set +a
    CUA_E2E_FLEET=1 .venv/bin/python -m pytest -q -s cua_bench/tests/live/test_cloud_live.py

What it proves, through the real CLI in a subprocess:

1. A 2-variant mini-batch with ``-j 2`` claims both sandboxes from ONE
   managed pool (sized ``max_pool_size = 2``) and completes.
2. A second batch reuses that same pool.
3. Every claim is released when the runs end; the pool stays (idle GC owns
   it). The test then GCs exactly the managed pools it used, through the
   SDK's scoped GC (by name, only once they have no claims), and verifies
   nothing is left.

The task needs no computer control, so it runs on the public daemon-agnostic
image (its own daemon on port 8000 is the readiness probe). Managed pools are
named ``cua-auto-<tenant/spec hash>``; the test tracks the ones its batches
used plus any new managed pool of its image, and automatic GC is off in the
CLI subprocesses, so no other pool is touched.

Image and runtime: ``CUA_BENCH_E2E_IMAGE`` / ``CUA_BENCH_E2E_RUNTIME``. The
default is the gVisor ``docker-*`` tag when the installed cua-sandbox selects
the Fleet runtime from the image kind, else the KubeVirt containerDisk.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
import textwrap
import time
from pathlib import Path

import pytest

AUTO_PREFIX = "cua-auto-"
GVISOR_IMAGE = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81"
KUBEVIRT_IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:82702ebdd32d1f8fc05f2ea409a7c67d0ba9f8f8e4e9f1a89ce40989d5f4475d"
)
SERVER_PORT = 8000
RUN_TIMEOUT_S = 1500

pytestmark = [
    pytest.mark.skipif(
        os.environ.get("CUA_E2E_FLEET") != "1",
        reason="opt-in: set CUA_E2E_FLEET=1 (creates billed Fleet sandboxes)",
    ),
    pytest.mark.skipif(
        not (
            os.environ.get("FLEETS_TOKEN")
            or (os.environ.get("CUA_CLIENT_ID") and os.environ.get("CUA_CLIENT_SECRET"))
        ),
        reason="Fleet credentials not set (set -a; source ~/.env; set +a)",
    ),
]

TASK = textwrap.dedent("""
    import cua_bench as cb

    @cb.tasks_config(split="train")
    def load():
        return [
            cb.Task(
                description=f"lifecycle {{i}}",
                computer={{"provider": "native", "setup_config": {{
                    "os_type": "linux", "image": {image!r}, "runtime": {runtime!r},
                    "server_port": {port}}}}},
            )
            for i in range(2)
        ]

    @cb.setup_task(split="train")
    async def setup(task_cfg, session):
        assert session.interface is not None  # a claimed, connected sandbox

    @cb.evaluate_task(split="train")
    async def evaluate(task_cfg, session):
        return [1.0]
    """)


def _defaults() -> tuple[str, str]:
    runtime = os.environ.get("CUA_BENCH_E2E_RUNTIME")
    image = os.environ.get("CUA_BENCH_E2E_IMAGE")
    if runtime is None:
        try:
            from cua_sandbox.transport.fleet_cloud import (  # noqa: F401
                resolve_fleet_runtime,
            )

            runtime = "container"
        except ImportError:
            runtime = "vm"
    if image is None:
        image = GVISOR_IMAGE if runtime == "container" else KUBEVIRT_IMAGE
    return image, runtime


def log(message: str) -> None:
    print(f"[bench-cloud-live {time.strftime('%H:%M:%S')}] {message}", flush=True)


async def _managed_pools() -> dict[str, "object"]:
    """This account's managed pools by name (the SDK's pool listing)."""
    from cua_sandbox import pools

    return {p.name: p for p in await pools.list_pools()}


async def _claims(pool: str) -> set[str]:
    """Claims still in ``pool`` (the SDK's public claim listing)."""
    from cua_sandbox import pools

    return {claim.name for claim in await pools.list_claims() if claim.pool == pool}


async def _gc_test_pools(names: set[str]) -> list[str]:
    """GC exactly ``names`` through the SDK (no claims left, idle now)."""
    from cua_sandbox import pools

    if not names:
        return []
    report = await pools.gc_pools(sorted(names), 0)
    for error in report.errors:
        log(f"cleanup: {error}")
    return report.pools_deleted + report.namespaces_deleted


def _cb(args: list[str], env: dict, cwd: Path) -> subprocess.CompletedProcess:
    cmd = [sys.executable, "-m", "cua_bench.cli.main", *args]
    log("$ cb " + " ".join(args))
    proc = subprocess.run(
        cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=RUN_TIMEOUT_S
    )
    for line in (proc.stdout + proc.stderr).splitlines():
        log(f"  | {line}")
    return proc


def _summary(output_dir: Path) -> dict:
    return json.loads((output_dir / "summary.json").read_text())


def test_cloud_mini_batch_reuses_one_managed_pool_and_cleans_up(tmp_path):
    from cua_bench.sandboxes import managed_pools_supported

    if not managed_pools_supported():
        pytest.skip("installed cua-sandbox has no managed Fleet pools yet")

    image, runtime = _defaults()
    log(f"image={image} runtime={runtime}")
    dataset = tmp_path / "dataset" / "lifecycle_env"
    dataset.mkdir(parents=True)
    (dataset / "main.py").write_text(TASK.format(image=image, runtime=runtime, port=SERVER_PORT))

    home = tmp_path / "home"
    home.mkdir()
    env = {
        **os.environ,
        "HOME": str(home),  # managed-pool cache, GC stamp and claim state stay off ~/.cua
        "XDG_DATA_HOME": str(tmp_path / "data"),
        "XDG_STATE_HOME": str(tmp_path / "state"),
        "CUA_FLEET_POOL_IDLE_GC": "off",
        "CUA_BENCH_NO_BANNER": "1",
        "PYTHONUNBUFFERED": "1",
    }
    runs: list[dict] = []
    # Scope cleanup to this test: pools its batches report, plus any managed
    # pool of its image that did not exist before (a crashed batch).
    before = set(asyncio.run(_managed_pools()))
    used: set[str] = set()
    try:
        for attempt in (1, 2):
            out = tmp_path / f"run{attempt}"
            proc = _cb(
                [
                    "run",
                    str(dataset.parent),
                    "--on",
                    "cloud",
                    "-j",
                    "2",
                    "--claim-ttl",
                    "5m",
                    "--output-dir",
                    str(out),
                ],
                env,
                tmp_path,
            )
            assert proc.returncode == 0, f"batch {attempt} failed (exit {proc.returncode})"
            summary = _summary(out)
            runs.append(summary)
            assert summary["completed"] == 2, summary
            pools = {r["pool"] for r in summary["results"]}
            used |= {p for p in pools if p}
            log(f"batch {attempt} pools: {pools}")
            assert len(pools) == 1, f"one managed pool per image, got {pools}"
            (pool,) = pools
            assert pool and pool.startswith(AUTO_PREFIX), pool
            if attempt == 1:
                assert "scaling up from zero" in proc.stdout or "sandbox ready" in proc.stdout

        first_pool = runs[0]["results"][0]["pool"]
        assert runs[1]["results"][0]["pool"] == first_pool, "second batch must reuse the pool"

        remaining = asyncio.run(_claims(first_pool))
        assert not remaining, f"claims left after the runs: {remaining}"
        assert first_pool in asyncio.run(_managed_pools()), "the managed pool stays for reuse"
    finally:
        now = asyncio.run(_managed_pools())
        used |= {
            n for n, p in now.items() if n not in before and getattr(p, "image", None) == image
        }
        leftover: list[str] = sorted(used)
        for _ in range(60):  # up to ~5 min; claim release and deletion are asynchronous
            deleted = asyncio.run(_gc_test_pools(set(leftover)))
            if deleted:
                log(f"deleted test pools {deleted}")
            leftover = sorted(set(leftover) & set(asyncio.run(_managed_pools())))
            if not leftover:
                break
            time.sleep(5)
        assert not leftover, f"test pools left behind: {leftover}"
