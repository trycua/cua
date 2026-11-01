"""E2B SDK conformance: one suite, any implementation.

Runs the unmodified `e2b` SDK against whatever its environment points at:

- the real E2B (`E2B_API_KEY` only),
- the Cua gateway (`eval "$(cua compat env --provider e2b)"`), or
- the `cua-e2b` shim (installed in place of `e2b`).

It prints one JSON object of *observations*: types, fields, values,
exception classes and messages, never timings or ids. `compare.py` diffs two
runs (or a run and a committed fixture) under the rules in `rules.json`.

    python conformance.py [--template T] [--only a,b] [--skip a,b] > run.json

Every check is independent: one failure is recorded and the suite goes on.
Every sandbox it creates is killed, even on failure.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import ssl
import time
import urllib.request

import e2b
from e2b import AsyncSandbox, Sandbox

ap = argparse.ArgumentParser()
ap.add_argument("--template", default=None)
ap.add_argument("--only", default="")
ap.add_argument("--skip", default="")
ap.add_argument("--scheme", default=os.environ.get("CONFORMANCE_SCHEME", "https"))
args = ap.parse_args()
ONLY = {s for s in args.only.split(",") if s}
SKIP = {s for s in args.skip.split(",") if s}

out: dict = {}
created: list = []


def version() -> str:
    try:
        from importlib.metadata import version as v

        return v("e2b")
    except Exception:  # noqa: BLE001
        return "?"


def exc_info(e: BaseException) -> dict:
    d = {
        "type": type(e).__name__,
        "mro": [c.__name__ for c in type(e).__mro__ if c.__module__.startswith("e2b")],
    }
    for k in ("exit_code", "stdout", "stderr", "error"):
        if hasattr(e, k):
            d[k] = getattr(e, k)
    d["message"] = str(e)
    return d


def wanted(name: str) -> bool:
    return (not ONLY or name in ONLY) and name not in SKIP


async def check(name, fn):
    if not wanted(name):
        return
    try:
        out[name] = {"ok": await fn()}
    except BaseException as e:  # noqa: BLE001
        out[name] = {"raised": exc_info(e)}


async def new_sandbox(**kw) -> AsyncSandbox:
    sb = await AsyncSandbox.create(template=args.template, **kw)
    created.append(sb)
    return sb


def fetch(url: str) -> object:
    ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
    last: object = None
    for _ in range(15):
        try:
            with urllib.request.urlopen(url, timeout=10, context=ctx) as r:
                return {"status": r.status, "body": r.read().decode()}
        except Exception as e:  # noqa: BLE001
            last = type(e).__name__
            time.sleep(1)
    return last


async def suite():
    sb = await new_sandbox(timeout=600, metadata={"suite": "conformance"})
    out["create"] = {"type": type(sb).__name__, "id_is_str": isinstance(sb.sandbox_id, str)}

    async def info():
        i = await sb.get_info()
        return {
            "state": str(i.state),
            "metadata": i.metadata,
            "type": type(i).__name__,
            "has_started_at": i.started_at is not None,
            "has_end_at": i.end_at is not None,
        }

    await check("get_info", info)
    await check("is_running", sb.is_running)

    async def listing():
        from e2b import SandboxQuery

        p = AsyncSandbox.list(query=SandboxQuery(metadata={"suite": "conformance"}))
        items = await p.next_items()
        return {
            "found": any(x.sandbox_id == sb.sandbox_id for x in items),
            "item_type": type(items[0]).__name__ if items else None,
        }

    await check("list_by_metadata", listing)

    # ── commands ─────────────────────────────────────────────────────────
    async def fg():
        r = await sb.commands.run("echo hi")
        return {
            "type": type(r).__name__,
            "stdout": r.stdout,
            "stderr": r.stderr,
            "exit_code": r.exit_code,
            "error": r.error,
        }

    await check("run_fg", fg)
    await check("run_nonzero", lambda: sb.commands.run("echo out; echo err >&2; exit 3"))
    await check("run_root", lambda: _stdout(sb.commands.run("id -u", user="root")))
    await check("run_default_user", lambda: _stdout(sb.commands.run("whoami; echo $HOME; pwd")))
    await check(
        "run_envs_cwd",
        lambda: _stdout(sb.commands.run('echo "$X:$PWD"', envs={"X": "1"}, cwd="/tmp")),
    )
    await check(
        "run_login_shell",
        lambda: _stdout(sb.commands.run("echo $0; shopt -q login_shell && echo login")),
    )
    await check("run_timeout", lambda: sb.commands.run("sleep 10", timeout=2))
    await check("run_bg_nonzero", lambda: _wait(sb.commands.run("exit 5", background=True)))

    async def callbacks():
        so, se = [], []
        r = await sb.commands.run("echo a; echo b >&2", on_stdout=so.append, on_stderr=se.append)
        return {"stdout": "".join(so), "stderr": "".join(se), "result": r.stdout + "|" + r.stderr}

    await check("run_callbacks", callbacks)

    async def bg():
        seen = []
        h = await sb.commands.run("sleep 1; echo done", background=True, on_stdout=seen.append)
        before = h.exit_code
        r = await h.wait()
        return {
            "handle_type": type(h).__name__,
            "pid_is_int": isinstance(h.pid, int),
            "exit_code_before": before,
            "result_type": type(r).__name__,
            "stdout": r.stdout,
            "callback": seen,
            "exit_code_after": h.exit_code,
        }

    await check("background", bg)

    async def bg_kill():
        h = await sb.commands.run("sleep 60", background=True, timeout=0)
        k = await h.kill()
        try:
            await h.wait()
            return {"kill": k, "wait": "returned"}
        except BaseException as e:  # noqa: BLE001
            return {"kill": k, "wait_raised": exc_info(e)}

    await check("background_kill", bg_kill)

    async def bg_disconnect():
        h = await sb.commands.run("sleep 3; echo late > /tmp/late.txt", background=True, timeout=0)
        await h.disconnect()
        await asyncio.sleep(5)
        return (await sb.commands.run("cat /tmp/late.txt")).stdout

    await check("background_disconnect_keeps_running", bg_disconnect)

    async def list_and_connect():
        h = await sb.commands.run("sleep 2; echo joined", background=True, timeout=0)
        procs = await sb.commands.list()
        listed = [p for p in procs if p.pid == h.pid]
        await h.disconnect()
        h2 = await sb.commands.connect(h.pid)
        r = await h2.wait()
        return {
            "listed": len(listed),
            "cmd": listed[0].cmd if listed else None,
            "args_tail": listed[0].args[-1:] if listed else None,
            "proc_type": type(listed[0]).__name__ if listed else None,
            "connected_stdout": r.stdout,
        }

    await check("list_and_connect", list_and_connect)
    await check("kill_unknown_pid", lambda: sb.commands.kill(999999))

    async def stdin():
        h = await sb.commands.run(
            "read line; echo got:$line", background=True, stdin=True, timeout=0
        )
        await sb.commands.send_stdin(h.pid, "hello\n")
        r = await h.wait()
        return r.stdout

    await check("send_stdin", stdin)

    async def pty():
        from e2b import PtySize

        data = []
        h = await sb.pty.create(PtySize(rows=24, cols=80), on_data=data.append, timeout=0)
        await sb.pty.send_stdin(h.pid, b"echo pty-$((1+2))\nexit\n")
        await asyncio.wait_for(h.wait(), 20)
        return {"saw_output": "pty-3" in b"".join(data).decode(errors="replace")}

    await check("pty", pty)

    # ── files ────────────────────────────────────────────────────────────
    async def files():
        w = await sb.files.write("/tmp/conf/a.txt", "héllo", user="root")
        text = await sb.files.read("/tmp/conf/a.txt", user="root")
        raw = await sb.files.read("/tmp/conf/a.txt", format="bytes")
        chunks = b""
        async with await sb.files.read(
            "/tmp/conf/a.txt", format="stream", user="root", request_timeout=60
        ) as s:
            async for c in s:
                chunks += c
        i = await sb.files.get_info("/tmp/conf/a.txt")
        await sb.files.remove("/tmp/conf/a.txt", user="root")
        return {
            "write_type": type(w).__name__,
            "write": [w.name, w.path, str(w.type)],
            "text": text,
            "bytes_type": type(raw).__name__,
            "stream": chunks.decode(),
            "info": {
                "type": str(i.type),
                "name": i.name,
                "path": i.path,
                "size": i.size,
                "permissions": i.permissions,
                "owner": i.owner,
                "mode": oct(i.mode),
            },
            "exists_after_remove": await sb.files.exists("/tmp/conf/a.txt"),
        }

    await check("files", files)

    async def files_default_user():
        w = await sb.files.write("conf-rel.txt", "x")
        i = await sb.files.get_info("conf-rel.txt")
        who = (await sb.commands.run("stat -c %U ~/conf-rel.txt")).stdout
        return {"path": w.path, "owner": i.owner, "stat_owner": who}

    await check("files_default_user", files_default_user)

    async def write_many():
        from e2b.sandbox.filesystem.filesystem import WriteEntry

        infos = await sb.files.write_files(
            [
                WriteEntry(path="/tmp/many/1.txt", data="one"),
                WriteEntry(path="/tmp/many/2.bin", data=b"\x00\x01"),
            ]
        )
        names = sorted(i.name for i in infos)
        b = await sb.files.read("/tmp/many/2.bin", format="bytes")
        return {"names": names, "bin": list(b)}

    await check("write_files", write_many)

    async def dirs():
        made = await sb.files.make_dir("/tmp/d1/d2")
        again = await sb.files.make_dir("/tmp/d1/d2")
        await sb.files.write("/tmp/d1/d2/f.txt", "f")
        entries = await sb.files.list("/tmp/d1")
        deep = await sb.files.list("/tmp/d1", depth=2)
        moved = await sb.files.rename("/tmp/d1/d2/f.txt", "/tmp/d1/g.txt")
        return {
            "made": made,
            "again": again,
            "list": sorted((e.name, str(e.type)) for e in entries),
            "deep": sorted(e.path for e in deep),
            "moved": [moved.name, moved.path, str(moved.type)],
            "old_exists": await sb.files.exists("/tmp/d1/d2/f.txt"),
        }

    await check("dirs", dirs)

    async def files_owner():
        await sb.files.write("/tmp/conf/root.txt", "1", user="root")
        await sb.files.write("/tmp/conf/user.txt", "1")
        root = (await sb.files.get_info("/tmp/conf/root.txt")).owner
        default = (await sb.files.get_info("/tmp/conf/user.txt")).owner
        return {"root": root, "default": default}

    await check("files_owner", files_owner)

    async def root_only_dir():
        await sb.files.write("/opt/conf-root/x.txt", "ok", user="root")
        return await sb.files.read("/opt/conf-root/x.txt", user="root")

    await check("files_write_root_only_dir", root_only_dir)
    await check("get_info_missing", lambda: sb.files.get_info("/tmp/nope-404"))

    async def file_metadata():
        w = await sb.files.write("/tmp/conf/m.txt", "m", metadata={"kind": "input"})
        return {"write": w.metadata, "info": (await sb.files.get_info("/tmp/conf/m.txt")).metadata}

    await check("file_metadata", file_metadata)
    await check("read_missing", lambda: sb.files.read("/tmp/nope-404"))
    await check("remove_missing", lambda: sb.files.remove("/tmp/nope-404"))
    await check("list_missing", lambda: sb.files.list("/tmp/nope-404"))

    async def watch():
        await sb.files.make_dir("/tmp/w")
        events = []
        h = await sb.files.watch_dir(
            "/tmp/w", on_event=lambda e: events.append((e.name, str(e.type)))
        )
        await asyncio.sleep(0.5)
        await sb.files.write("/tmp/w/new.txt", "n")
        for _ in range(50):
            if any(n == "new.txt" for n, _ in events):
                break
            await asyncio.sleep(0.2)
        await h.stop()
        return {"saw_create_or_write": any(n == "new.txt" for n, _ in events)}

    await check("watch_dir", watch)

    async def signed_urls():
        await sb.files.write("/tmp/signed.txt", "signed", user="root")
        url = sb.download_url("/tmp/signed.txt", user="root", use_signature_expiration=120)
        got = await asyncio.to_thread(fetch, url)
        return {"download": got}

    await check("download_url", signed_urls)

    # ── ports ────────────────────────────────────────────────────────────
    async def host():
        await sb.files.write("/tmp/www/index.html", "hello-from-sandbox", user="root")
        await sb.commands.run(
            "python3 -m http.server 8080", background=True, cwd="/tmp/www", user="root", timeout=0
        )
        await asyncio.sleep(2)
        h = sb.get_host(8080)
        fmt = re.sub(r"[0-9a-z]{12,}", "<id>", h)
        got = await asyncio.to_thread(fetch, f"{args.scheme}://{h}/")
        return {
            "format_has_port_prefix": fmt.startswith("8080-<id>."),
            "has_scheme": "://" in h,
            "fetch": got,
        }

    await check("get_host", host)

    async def metrics():
        ms = await sb.get_metrics()
        return {"type": type(ms).__name__, "item": type(ms[0]).__name__ if ms else None}

    await check("get_metrics", metrics)
    await check("set_timeout", lambda: sb.set_timeout(600))

    async def reconnect():
        again = await AsyncSandbox.connect(sb.sandbox_id)
        return (await again.commands.run("echo reconnected")).stdout

    await check("connect_by_id", reconnect)

    # ── snapshots, pause, fork ───────────────────────────────────────────
    async def snapshot_roundtrip():
        await sb.files.write("/tmp/world.txt", "baked", user="root")
        snap = await sb.create_snapshot()
        listed = [s for s in await _all_snapshots(sb)]
        child = await AsyncSandbox.create(template=snap.snapshot_id, timeout=300)
        created.append(child)
        content = await child.files.read("/tmp/world.txt", user="root")
        await child.kill()
        deleted = await AsyncSandbox.delete_snapshot(snap.snapshot_id)
        return {
            "type": type(snap).__name__,
            "listed": snap.snapshot_id in listed,
            "child_sees_file": content,
            "deleted": deleted,
        }

    await check("snapshot_create_fork", snapshot_roundtrip)

    async def pause_resume():
        other = await new_sandbox(timeout=300)
        await other.files.write("/tmp/kept.txt", "kept", user="root")
        paused = await other.pause()
        state = str((await other.get_info()).state)
        again = await AsyncSandbox.connect(other.sandbox_id, timeout=300)
        content = await again.files.read("/tmp/kept.txt", user="root")
        await again.kill()
        return {"paused": paused, "state_while_paused": state, "after_resume": content}

    await check("pause_resume", pause_resume)

    async def no_internet():
        off = await new_sandbox(timeout=300, allow_internet_access=False)
        probe = "python3 -c \"import socket; socket.create_connection(('1.1.1.1', 443), 5)\""
        try:
            await off.commands.run(probe, timeout=30)
            outbound = "connected"
        except e2b.CommandExitException:
            outbound = "refused"
        runs = (await off.commands.run("echo ok")).stdout
        await off.kill()
        return {"outbound": outbound, "runs": runs}

    await check("no_internet", no_internet)

    # ── sync API ─────────────────────────────────────────────────────────
    def sync_flow():
        s = Sandbox.create(template=args.template, timeout=300)
        try:
            r = s.commands.run("echo sync")
            s.files.write("/tmp/s.txt", "s")
            return {
                "type": type(s).__name__,
                "stdout": r.stdout,
                "read": s.files.read("/tmp/s.txt"),
            }
        finally:
            s.kill()

    await check("sync_api", lambda: asyncio.to_thread(sync_flow))

    # ── kill and lifetime ────────────────────────────────────────────────
    out["kill"] = await sb.kill()
    await check("kill_again", sb.kill)
    await check("run_after_kill", lambda: sb.commands.run("echo x", request_timeout=20))

    async def expiry():
        short = await new_sandbox(timeout=5)
        await asyncio.sleep(14)
        return await short.commands.run("echo alive", request_timeout=20)

    await check("lifetime_expired_run", expiry)


async def _all_snapshots(sb) -> list:
    p = sb.list_snapshots()
    ids = []
    while True:
        items = await p.next_items()
        ids += [i.snapshot_id for i in items]
        if not p.has_next:
            return ids


async def _stdout(coro):
    return (await coro).stdout


async def _wait(coro):
    return await (await coro).wait()


async def main():
    out["sdk_version"] = version()
    try:
        await suite()
    finally:
        for s in created:
            try:
                await s.kill()
            except BaseException:  # noqa: BLE001
                pass
    print(json.dumps(out, indent=1, sort_keys=True, default=str))


asyncio.run(main())
