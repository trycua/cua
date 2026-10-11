"""OSWorld eval parity, locally, through cua-bench's real runner and adapter.

    python osworld_parity.py INDEX_REF OUT_DIR [N_TASKS]

For each variant (container on runc: the amd64 image under emulation on an
arm64 host; vm: the disk index under QEMU/TCG) and each mode (oracle, noop),
runs the first N parity tasks through BatchRunner with an opener that starts
a fresh local sandbox per task (cua-e2e-bench-osw-*), then prints a table.
"""

import asyncio
import json
import os
import sys
import uuid
from contextlib import asynccontextmanager
from pathlib import Path

from cua_sandbox import Image, Sandbox

REPO = Path(__file__).resolve().parents[2]
TASK = REPO / "libs/cua-bench/tasks/osworld"


async def main() -> int:
    ref, out = sys.argv[1], Path(sys.argv[2])
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 2
    os.environ["CUA_BENCH_OSWORLD_SPLIT"] = "parity"
    os.environ["CUA_BENCH_IMAGE_BENCH_OSWORLD"] = ref
    from cua_bench.runner import AgentOptions, BatchRunner, Job
    from cua_bench.targets import Target, resolve_env_spec
    from cua_bench import make

    env = make(str(TASK), split="train")
    tasks = env.tasks_config_fn()[:n]
    rows = []
    for variant in ("container", "vm"):

        @asynccontextmanager
        async def opener(spec, target, *, max_pool_size=1, on_progress=None, _v=variant):
            img = Image.from_registry(ref, kind=_v)
            for p in (5000, 9222, 8080):
                img = img.expose(p)
            extra = {"runtime": "runc"} if _v == "container" else {}
            async with Sandbox.ephemeral(img, local=True, name=f"cua-e2e-bench-osw-{uuid.uuid4().hex[:6]}",
                                         server_port=5000, cpu=4, memory_mb=4096, time_to_start=1800,
                                         telemetry_enabled=False, **extra) as sb:
                yield sb

        target = Target(on="local", concurrency=1)
        for mode in ("oracle", "noop"):
            opts = AgentOptions(oracle=(mode == "oracle"), dump=(mode == "noop"))
            jobs = []
            for i, t in enumerate(tasks):
                spec = resolve_env_spec(t.computer, target)
                jobs.append(Job(TASK, i, spec, f"osw-{variant}-{mode}-{i}", out / f"{variant}-{mode}" / f"v{i}"))
            results = await BatchRunner(target, opts, opener=opener).run(jobs)
            for i, r in enumerate(results):
                rows.append({"variant": variant, "mode": mode, "task": tasks[i].metadata.get("id") if tasks[i].metadata else i,
                             "status": r.status, "reward": r.reward, "image_digest": r.image_digest,
                             "image_variant": r.image_variant, "error": (r.error or "")[:300]})
                print(json.dumps(rows[-1]), flush=True)
    (out / "parity.json").write_text(json.dumps({"image": ref, "rows": rows}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
