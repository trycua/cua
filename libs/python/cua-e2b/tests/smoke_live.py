"""An RL-rollout-shaped smoke against real Spaces (opt-in; boots VMs).

1. Bake: create a VM sandbox, start an HTTP service in it, checkpoint it.
2. Rollout: create(template=<checkpoint>) forks it; the service is already
   running (a live memory snapshot), reached with get_host.
3. A command loop in the Collector style (background, timeout=0, the reply
   parsed from on_stdout, then disconnect), foreground commands, a
   CommandExitException, and a file round trip (write / read / stream /
   remove).
4. kill both; print a JSON report with timings.

    CUA_HOME=... python smoke_live.py
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shlex
import time
import urllib.request

from e2b import AsyncSandbox, CommandExitException

report: dict = {}


def lap(name, t0):
    report.setdefault("timings_s", {})[name] = round(time.monotonic() - t0, 2)


async def collector_call(sb, payload: dict) -> dict:
    """An RL tool collector call: a background command whose
    first stdout line is the JSON reply."""
    reply = asyncio.get_running_loop().create_future()
    chunks = []

    def receive(text):
        if reply.done():
            return
        line, sep, _ = text.partition("\n")
        chunks.append(line)
        if sep:
            reply.set_result(json.loads("".join(chunks)))

    script = (
        "import json,sys,time; req=json.loads(sys.argv[1]); "
        "print(json.dumps({'echo': req, 'pid_ok': True}), flush=True); time.sleep(2)"
    )
    cmd = shlex.join(["python3", "-c", script, json.dumps(payload)])
    handle = await sb.commands.run(cmd, background=True, timeout=0, user="root", on_stdout=receive)
    waiter = asyncio.create_task(handle.wait())
    try:
        await asyncio.wait((reply, waiter), return_when=asyncio.FIRST_COMPLETED)
        if reply.done():
            return reply.result()
        waiter.result()
        raise RuntimeError("Tool stream ended without a result")
    finally:
        await handle.disconnect()
        await asyncio.gather(waiter, return_exceptions=True)


async def http_get(host: str) -> int:
    def get():
        return urllib.request.urlopen(f"http://{host}/marker.txt", timeout=10).read().decode()

    for _ in range(20):
        try:
            return await asyncio.to_thread(get)
        except Exception:  # noqa: BLE001
            await asyncio.sleep(1)
    raise RuntimeError("service not reachable through get_host")


async def main():
    template = os.environ.get("SMOKE_IMAGE")  # None: canonical Linux image
    ckpt = f"smoke-world-{os.getpid()}"
    created = []
    try:
        # 1. bake
        t0 = time.monotonic()
        bake = await AsyncSandbox.create(template=template, timeout=1800)
        created.append(bake)
        lap("bake_create", t0)
        report["bake_id"] = bake.sandbox_id
        report["uname"] = (await bake.commands.run("uname -srm")).stdout.strip()
        report["docker"] = (
            await bake.commands.run(
                "command -v docker && docker --version || echo none", user="root"
            )
        ).stdout.strip()
        await bake.files.write("/srv/world/marker.txt", f"world {ckpt}\n", user="root")
        await bake.commands.run(
            "python3 -m http.server 8765", background=True, cwd="/srv/world", user="root", timeout=0
        )
        await asyncio.sleep(1)
        report["bake_get_host"] = bake.get_host(8765)
        report["bake_http"] = await http_get(report["bake_get_host"])
        t0 = time.monotonic()
        snap = await bake.create_snapshot(ckpt)
        lap("checkpoint", t0)
        report["snapshot"] = snap.snapshot_id

        # 2. rollout from the live snapshot
        t0 = time.monotonic()
        sb = await AsyncSandbox.create(template=snap.snapshot_id, timeout=1800)
        created.append(sb)
        lap("fork_create", t0)
        report["rollout_id"] = sb.sandbox_id
        host = sb.get_host(8765)
        report["rollout_get_host"] = host
        t0 = time.monotonic()
        report["rollout_http_from_snapshot"] = await http_get(host)
        lap("rollout_http", t0)

        # 3. command loop
        t0 = time.monotonic()
        replies = [await collector_call(sb, {"step": i}) for i in range(5)]
        lap("collector_5_calls", t0)
        report["collector_ok"] = [r["echo"]["step"] for r in replies] == list(range(5))
        t0 = time.monotonic()
        for i in range(20):
            r = await sb.commands.run(f"echo {i}", user="root", timeout=30)
            assert r.stdout == f"{i}\n"
        lap("fg_20_calls", t0)
        try:
            await sb.commands.run("echo boom >&2; exit 7", user="root")
            report["exit_exception"] = "not raised"
        except CommandExitException as e:
            report["exit_exception"] = {
                "exit_code": e.exit_code,
                "stderr": e.stderr,
                "error": e.error,
            }

        payload = os.urandom(8 * 1024 * 1024)
        t0 = time.monotonic()
        await sb.files.write("/tmp/rollout/inputs.tar", payload, user="root")
        lap("write_8MiB", t0)
        size = (
            await sb.commands.run("stat -c %s /tmp/rollout/inputs.tar", user="root")
        ).stdout.strip()
        t0 = time.monotonic()
        digest = hashlib.sha256()
        async with await sb.files.read(
            "/tmp/rollout/inputs.tar", format="stream", user="root", request_timeout=60
        ) as stream:
            async for chunk in stream:
                digest.update(chunk)
        lap("read_stream_8MiB", t0)
        await sb.files.remove("/tmp/rollout/inputs.tar", user="root")
        report["file_round_trip"] = {
            "guest_size": size,
            "sha256_match": digest.hexdigest() == hashlib.sha256(payload).hexdigest(),
            "exists_after_remove": await sb.files.exists("/tmp/rollout/inputs.tar"),
        }
    finally:
        # 4. teardown
        report["kill"] = [await s.kill() for s in created]
        try:
            report["checkpoint_deleted"] = await AsyncSandbox.delete_snapshot(ckpt)
            report["checkpoint_listed_after"] = [
                i.snapshot_id for i in await AsyncSandbox.list_snapshots().next_items()
            ]
        except Exception as e:  # noqa: BLE001
            report["checkpoint_deleted"] = repr(e)
    print(json.dumps(report, indent=1))


asyncio.run(main())
