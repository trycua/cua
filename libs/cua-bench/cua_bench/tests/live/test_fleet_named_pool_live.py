"""LIVE: cua-bench's runner on a Fleet gVisor sandbox (opt-in, billed).

Run only on purpose::

    set -a; source ~/.env; set +a
    CUA_E2E_FLEET=1 .venv/bin/python -m pytest -q -s cua_bench/tests/live/test_fleet_named_pool_live.py

It applies one gVisor pool named ``cua-e2e-bench-<random>`` for the
canonical Linux image (spacesd, env token delivered through the claim's
Secret), runs ``hello_file_env`` (both variants) through cua-bench's
BatchRunner on claims from it, checks the rewards and the recorded
runtime/image facts, and deletes the pool in ``finally``.

``cb run --on cloud`` (managed ``cua-auto-*`` pools) is covered by
``test_cloud_live.py``.
"""

from __future__ import annotations

import asyncio
import json
import os
import uuid
from contextlib import asynccontextmanager
from pathlib import Path

import pytest

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

HELLO = Path(__file__).resolve().parents[3] / "example_tasks" / "hello_file_env"
IMAGE = os.environ.get("CUA_BENCH_E2E_FLEET_IMAGE", "ghcr.io/trycua/linux:24.04")
TIMEOUT_S = 1800


def log(message: str) -> None:
    print(f"[bench-fleet-live] {message}", flush=True)


async def _run(tmp_path: Path) -> list:
    from cua_bench.runner import AgentOptions, BatchRunner, Job
    from cua_bench.targets import Target, resolve_env_spec
    from cua_sandbox import Image, Pool, PoolOptions, SandboxSpec, generate_claim_token

    name = f"cua-e2e-bench-{uuid.uuid4().hex[:8]}"
    log(f"applying pool {name} ({IMAGE}, gvisor)")
    pool = await Pool.apply(
        name,
        SandboxSpec(
            image=Image.from_registry(IMAGE, kind="container"),
            cpu=2,
            memory_mb=4096,
            services={"env": 3211},  # cua-spacesd
            claim_secrets=True,
        ),
        PoolOptions(runtime="gvisor", replicas=0, max_pool_size=2),
    )
    claims = 0
    try:

        @asynccontextmanager
        async def opener(spec, target, *, max_pool_size=1, on_progress=None):
            nonlocal claims
            claims += 1
            claim = f"{name}-c{claims}"
            log(f"claiming {claim}")
            async with pool.claim(
                name=claim,
                claim_token=generate_claim_token(),
                time_to_start=900,
                ttl_seconds_after_created=3600,
            ) as sandbox:
                yield sandbox
            log(f"released {claim}")

        target = Target(on="cloud", concurrency=2)
        spec = resolve_env_spec(
            {"provider": "native", "setup_config": {"os_type": "linux"}}, target
        )
        jobs = [
            Job(HELLO, v, spec, f"{name}-v{v}", tmp_path / f"hello_file_env_v{v}") for v in (0, 1)
        ]
        runner = BatchRunner(target, AgentOptions(), opener=opener)
        return await asyncio.wait_for(runner.run(jobs), TIMEOUT_S)
    finally:
        log(f"deleting pool {name}")
        await pool.delete()


def test_fleet_gvisor_hello_file_env(tmp_path):
    results = asyncio.run(_run(tmp_path))
    for result in results:
        log(f"{result.task} v{result.variant}: {result.status} reward={result.reward}")
        print((Path(result.output_dir) / "run.log").read_text()[-3000:])
    assert [r.status for r in results] == ["completed", "completed"]
    assert all(r.reward == 1.0 for r in results)
    saved = json.loads((tmp_path / "hello_file_env_v0" / "result.json").read_text())
    assert saved["on"] == "cloud" and saved["backend"] == "cloud-gvisor"
    assert saved["runtime"] == "container"
    log(f"image_digest={saved['image_digest']} arch={saved['arch']}")
    # The pool's template image, pinned by the SDK (Fleet.pool_image_info).
    assert "@sha256:" in (saved["image_digest"] or ""), saved
