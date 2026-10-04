#!/usr/bin/env python3
"""Eval parity on Fleet for one benchmark image (opt-in, billed).

    fleet_eval.py IMAGE_REF gvisor|kubevirt TASK_DIR SERVER_PORT EXTRA_PORTS OUT [K=V ...]

Applies a pool ``cua-e2e-bench-<rand>`` from IMAGE_REF (a digest-pinned
index; gVisor runs its rootfs, KubeVirt its containerDisk), runs
``cb run dataset TASK_DIR --oracle`` and then ``--noop`` against it with
``--image pool:<name> --cloud``, and deletes the pool in ``finally``.
K=V pairs are added to the environment (e.g. CUA_BENCH_MINIWOB_TASKS=...).
Needs Fleet credentials (CUA_CLIENT_ID/SECRET or FLEETS_TOKEN) and an image
Fleet can pull (public, or a pool registry secret).
"""

import asyncio
import os
import subprocess
import sys
import uuid

from cua_sandbox import Image, Pool, PoolOptions, SandboxSpec


async def main() -> int:
    ref, runtime, task_dir, server_port, extra, out = sys.argv[1:7]
    env_over = dict(kv.split("=", 1) for kv in sys.argv[7:])
    server_port = int(server_port)
    ports = [int(p) for p in extra.split(",") if p]
    kind = "container" if runtime == "gvisor" else "vm"
    name = f"cua-e2e-bench-{uuid.uuid4().hex[:6]}"
    services = {"env": 3211, "server": server_port}
    services.update({f"port-{p}": p for p in ports})
    image = Image.from_registry(ref, kind=kind)
    print(f"[fleet-eval] pool {name} {runtime} {ref}", flush=True)
    pool = await Pool.apply(
        name,
        SandboxSpec(image=image, cpu=4, memory_mb=8192, services=services, claim_secrets=True),
        PoolOptions(runtime=runtime, replicas=0, max_pool_size=2),
    )
    rc = 0
    try:
        for mode in ("--oracle", "--noop"):
            cmd = ["cb", "run", "dataset", task_dir, mode, "--image", f"pool:{name}", "--cloud",
                   "-j", "1", "--output-dir", f"{out}/{runtime}{mode}"]
            print("[fleet-eval] $", " ".join(cmd), flush=True)
            r = subprocess.run(cmd, env={**os.environ, **env_over}, timeout=5400)
            rc = rc or r.returncode
    finally:
        print(f"[fleet-eval] deleting pool {name}", flush=True)
        await pool.delete()
    return rc


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
