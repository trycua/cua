"""fork-batch (qemu lane): the reference disk booted through
``Sandboxes.create(local)``, checkpointed with its memory, then
``Sandboxes.fork_many`` of that checkpoint: forks stream as they become
ready, at most ``max_parallel_per_host`` start at once, and each fork's
cua-spacesd answers with its own token. Everything it makes is deleted.
"""

from __future__ import annotations

import os
import secrets
from pathlib import Path

import e2e
import pytest

import cua


def _disk() -> Path:
    return Path(
        os.environ.get(
            "CUA_E2E_DISK_LINUX",
            str(Path.home() / ".cache" / "cua-images-e2e" / "linux" / e2e.HOST_ARCH / "disk.img"),
        )
    )


@pytest.mark.e2e("fork-batch", "qemu")
def test_fork_many_from_a_memory_checkpoint(local_cua):
    disk = _disk()
    if not disk.exists():
        pytest.skip(f"missing {disk}")
    sbx = local_cua.sandboxes()
    src = e2e.name("fb-src")
    ck = f"{src}-ck"
    token = secrets.token_hex(16)

    async def body():
        forks = []
        try:
            sb = await sbx.create(
                cua.SandboxCreateOptions(
                    on="local",
                    image=f"vm:{disk}",
                    name=src,
                    token=token,
                    env={"CUA_ENV_TOKEN": token},
                    cpus=2,
                    memory_mb=1536,
                    ports=[3211],
                    wait_for=[cua.ReadinessProbe(port=3211, http_path="/viewer/")],
                    ready_timeout_ms=600_000,
                )
            )
            await e2e.wait_env(sb, 300)
            await sbx.checkpoint(src, ck)
            batch = await sbx.fork_many(
                ck,
                3,
                cua.ForkManyOptions(on="local", max_parallel_per_host=2, name_prefix=f"{src}-f"),
            )
            items, summary = [], None
            while (ev := await batch.next()) is not None:
                if ev.is_item():
                    assert summary is None, "items stream before the summary"
                    forks.append(ev.item.name)
                    assert ev.item.error == "", ev.item
                    items.append(ev.item)
                elif ev.is_done():
                    summary = ev.summary
            assert summary is not None
            assert (summary.ok, summary.failed) == (3, 0), summary
            assert summary.peak_parallel.get("local", 0) <= 2, summary
            for item in items:
                assert item.space == f"local:{item.name}"
                fork = await sbx.connect(item.name)
                env = await e2e.wait_env(fork, 120)
                await e2e.sh_ok(env, "true")
        finally:
            for f in forks:
                try:
                    await sbx.delete(f)
                except cua.CuaError:
                    pass
            try:
                await sbx.delete_checkpoint(ck)
            except cua.CuaError:
                pass
            try:
                await sbx.delete(src)
            except cua.CuaError:
                pass

    e2e.run_async(body(), timeout=2400)
