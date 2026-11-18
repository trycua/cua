"""The unmodified `e2b` SDK against the gateway over mock Spaces.

Hermetic (no VMs, nothing runs on the host): the gateway's backend is the
in-process mock cua-spacesd, whose processes are simulated (`echo`, `fail N`,
`ticker N MS`, `cat`, `sleep MS`) and whose files live in memory. A guest
port is stood in for by a local HTTP server the runner maps to guest 8080.

    run_hermetic.sh             # builds the mock gateway, runs this per SDK

Each check asserts SDK-visible behaviour; any failure exits 1.
"""

from __future__ import annotations

import asyncio
import json
import os
import ssl
import sys
import time
import traceback
import urllib.request

env = json.loads(open(sys.argv[1]).read())
os.environ.update(env)

import e2b  # noqa: E402
from e2b import (  # noqa: E402
    AsyncSandbox,
    CommandExitException,
    FileNotFoundException,
    Sandbox,
    TimeoutException,
)

failures: list[str] = []
passed = 0


async def check(name, fn):
    global passed
    try:
        await fn()
        passed += 1
        print(f"PASS {name}")
    except BaseException as e:  # noqa: BLE001
        failures.append(name)
        print(f"FAIL {name}: {type(e).__name__}: {e}")
        traceback.print_exc()


def fetch(url: str) -> tuple[int, str]:
    ctx = ssl.create_default_context(cafile=os.environ["SSL_CERT_FILE"])
    with urllib.request.urlopen(url, timeout=10, context=ctx) as r:
        return r.status, r.read().decode()


async def main():
    sb = await AsyncSandbox.create(timeout=120, metadata={"suite": "hermetic"})
    try:
        await run(sb)
    finally:
        await sb.kill()
    print(f"{passed} passed, {len(failures)} failed: {failures}")
    sys.exit(1 if failures else 0)


async def run(sb: AsyncSandbox):
    async def info():
        i = await sb.get_info()
        assert str(i.state) in ("running", "SandboxState.RUNNING"), i.state
        assert i.metadata == {"suite": "hermetic"}, i.metadata
        assert await sb.is_running()
        from e2b import SandboxQuery

        items = await AsyncSandbox.list(
            query=SandboxQuery(metadata={"suite": "hermetic"})
        ).next_items()
        assert any(x.sandbox_id == sb.sandbox_id for x in items)
        none = await AsyncSandbox.list(query=SandboxQuery(metadata={"suite": "other"})).next_items()
        assert not any(x.sandbox_id == sb.sandbox_id for x in none)

    await check("info_and_list", info)

    async def fg():
        r = await sb.commands.run("echo hi there")
        assert (r.stdout, r.stderr, r.exit_code, r.error) == ("hi there\n", "", 0, None), r

    await check("run_foreground", fg)

    async def nonzero():
        try:
            await sb.commands.run("fail 3")
        except CommandExitException as e:
            assert e.exit_code == 3 and e.error == "exit status 3", (e.exit_code, e.error)
            assert e.stderr, e.stderr
            return
        raise AssertionError("no exception")

    await check("run_nonzero", nonzero)

    async def background():
        seen = []
        h = await sb.commands.run("ticker 3 50", background=True, on_stdout=seen.append)
        assert isinstance(h.pid, int)
        r = await h.wait()
        assert r.stdout == "tick 0\ntick 1\ntick 2\n", r.stdout
        assert "".join(seen) == r.stdout, seen

    await check("background_callbacks", background)

    async def bg_kill():
        h = await sb.commands.run("sleep 60000", background=True, timeout=0)
        assert await h.kill() is True
        try:
            await h.wait()
        except CommandExitException as e:
            assert e.exit_code == -1 and e.error == "signal: killed", (e.exit_code, e.error)
            return
        raise AssertionError("wait returned after kill")

    await check("background_kill", bg_kill)

    async def list_connect():
        h = await sb.commands.run("ticker 4 300", background=True, timeout=0)
        procs = await sb.commands.list()
        assert any(p.pid == h.pid for p in procs), procs
        await h.disconnect()
        h2 = await sb.commands.connect(h.pid)
        r = await h2.wait()
        assert r.exit_code == 0

    await check("list_disconnect_connect", list_connect)

    async def kill_unknown():
        assert await sb.commands.kill(999999) is False

    await check("kill_unknown_pid", kill_unknown)

    async def stdin():
        out = []
        h = await sb.commands.run(
            "cat", background=True, stdin=True, timeout=0, on_stdout=out.append
        )
        await sb.commands.send_stdin(h.pid, "ping\n")
        for _ in range(50):
            if "".join(out) == "ping\n":
                break
            await asyncio.sleep(0.05)
        assert "".join(out) == "ping\n", out
        await h.kill()

    await check("send_stdin", stdin)

    async def timeout():
        t = time.monotonic()
        try:
            await sb.commands.run("sleep 10000", timeout=1)
        except TimeoutException:
            assert time.monotonic() - t < 5
            return
        raise AssertionError("no timeout")

    await check("run_timeout", timeout)

    async def files():
        w = await sb.files.write("/tmp/a.txt", "héllo")
        assert (w.name, w.path, w.type.value) == ("a.txt", "/tmp/a.txt", "file"), w
        assert await sb.files.read("/tmp/a.txt") == "héllo"
        assert await sb.files.read("/tmp/a.txt", format="bytes") == bytearray("héllo".encode())
        chunks = b""
        async with await sb.files.read("/tmp/a.txt", format="stream") as s:
            async for c in s:
                chunks += c
        assert chunks.decode() == "héllo"
        i = await sb.files.get_info("/tmp/a.txt")
        assert (i.name, i.size, i.type.value) == ("a.txt", 6, "file"), i
        assert await sb.files.exists("/tmp/a.txt") and not await sb.files.exists("/tmp/nope")
        await sb.files.remove("/tmp/a.txt")
        assert not await sb.files.exists("/tmp/a.txt")
        await sb.files.remove("/tmp/a.txt")  # missing: no error, like envd

    await check("files_roundtrip", files)

    async def big_and_many():
        from e2b.sandbox.filesystem.filesystem import WriteEntry

        blob = os.urandom(3 * 1024 * 1024 + 7)
        await sb.files.write("/tmp/big.bin", blob)
        assert bytes(await sb.files.read("/tmp/big.bin", format="bytes")) == blob
        import io

        await sb.files.write("/tmp/stream.bin", io.BytesIO(blob))
        assert bytes(await sb.files.read("/tmp/stream.bin", format="bytes")) == blob
        infos = await sb.files.write_files(
            [
                WriteEntry(path="/tmp/m/1.txt", data="one"),
                WriteEntry(path="/tmp/m/2.bin", data=b"\x00\x01"),
            ]
        )
        assert sorted(i.name for i in infos) == ["1.txt", "2.bin"]
        assert await sb.files.read("/tmp/m/1.txt") == "one"

    await check("files_large_streamed_and_multipart", big_and_many)

    async def read_missing():
        try:
            await sb.files.read("/tmp/nope-404")
        except FileNotFoundException as e:
            assert str(e) == "path '/tmp/nope-404' does not exist", str(e)
            return
        raise AssertionError("no exception")

    await check("read_missing", read_missing)

    async def dirs():
        assert await sb.files.make_dir("/tmp/d1/d2") is True
        assert await sb.files.make_dir("/tmp/d1/d2") is False
        await sb.files.write("/tmp/d1/d2/f.txt", "f")
        names = sorted(e.name for e in await sb.files.list("/tmp/d1"))
        assert "d2" in names, names
        moved = await sb.files.rename("/tmp/d1/d2/f.txt", "/tmp/d1/g.txt")
        assert moved.path == "/tmp/d1/g.txt", moved
        assert not await sb.files.exists("/tmp/d1/d2/f.txt")

    await check("dirs_list_rename", dirs)

    async def signed():
        await sb.files.write("/tmp/signed.txt", "signed")
        url = sb.download_url("/tmp/signed.txt", use_signature_expiration=60)
        status, body = await asyncio.to_thread(fetch, url)
        assert (status, body) == (200, "signed")
        bad = url.replace("signature=v1_", "signature=v1_x")
        try:
            await asyncio.to_thread(fetch, bad)
        except urllib.error.HTTPError as e:
            assert e.code == 401
            return
        raise AssertionError("tampered signature accepted")

    await check("signed_download_url", signed)

    async def host():
        h = sb.get_host(8080)
        assert h.startswith(f"8080-{sb.sandbox_id}."), h
        for scheme in ("https", "http"):
            status, body = await asyncio.to_thread(fetch, f"{scheme}://{h}/")
            assert (status, body) == (200, "hello-from-port-8080"), (scheme, status, body)

    await check("get_host_https_and_http", host)

    async def metrics():
        ms = await sb.get_metrics()
        assert isinstance(ms, list)

    await check("get_metrics", metrics)

    async def timeouts_and_connect():
        await sb.set_timeout(300)
        again = await AsyncSandbox.connect(sb.sandbox_id)
        r = await again.commands.run("echo again")
        assert r.stdout == "again\n"

    await check("set_timeout_and_connect", timeouts_and_connect)

    async def snapshot():
        await sb.files.write("/tmp/world.txt", "baked")
        snap = await sb.create_snapshot()
        ids = [s.snapshot_id for s in await sb.list_snapshots().next_items()]
        assert snap.snapshot_id in ids, ids
        child = await AsyncSandbox.create(template=snap.snapshot_id, timeout=60)
        try:
            assert await child.files.read("/tmp/world.txt") == "baked"
        finally:
            await child.kill()
        assert await AsyncSandbox.delete_snapshot(snap.snapshot_id) is True
        assert await AsyncSandbox.delete_snapshot(snap.snapshot_id) is False

    await check("snapshot_then_create_from_it", snapshot)

    async def fork():
        await sb.files.write("/tmp/forked.txt", "shared")
        kids = await sb.fork(count=2, timeout=60)
        assert len(kids) == 2 and all(isinstance(k, AsyncSandbox) for k in kids), kids
        try:
            for k in kids:
                assert await k.files.read("/tmp/forked.txt") == "shared"
        finally:
            for k in kids:
                await k.kill()

    await check("fork_count", fork)

    async def pause():
        other = await AsyncSandbox.create(timeout=60)
        try:
            await other.files.write("/tmp/kept.txt", "kept")
            assert await other.pause() is True
            assert str((await other.get_info()).state) in ("paused", "SandboxState.PAUSED")
            again = await AsyncSandbox.connect(other.sandbox_id, timeout=60)
            assert await again.files.read("/tmp/kept.txt") == "kept"
        finally:
            await other.kill()

    await check("pause_and_resume_on_connect", pause)

    def sync_flow():
        s = Sandbox.create(timeout=60)
        try:
            assert s.commands.run("echo sync").stdout == "sync\n"
            s.files.write("/tmp/s.txt", "s")
            assert s.files.read("/tmp/s.txt") == "s"
        finally:
            assert s.kill() is True

    await check("sync_api", lambda: asyncio.to_thread(sync_flow))

    async def unsupported():
        try:
            await AsyncSandbox.create(timeout=60, mcp={"x": {}})
        except Exception as e:  # noqa: BLE001
            assert "mcp" in str(e), str(e)
            return
        raise AssertionError("mcp accepted")

    await check("unsupported_field_is_refused", unsupported)

    async def expiry():
        short = await AsyncSandbox.create(timeout=1)
        # The reaper ticks on its own clock: give a loaded runner 20 s.
        for _ in range(20):
            await asyncio.sleep(1)
            try:
                await short.commands.run("echo alive", request_timeout=10)
            except TimeoutException as e:
                assert str(e).startswith("The sandbox was not found"), str(e)
                return
        raise AssertionError("sandbox outlived its timeout")

    await check("lifetime_expiry", expiry)

    async def kill_twice():
        other = await AsyncSandbox.create(timeout=60)
        assert await other.kill() is True
        assert await other.kill() is False
        try:
            await other.commands.run("echo x", request_timeout=10)
        except TimeoutException:
            return
        raise AssertionError("ran after kill")

    await check("kill_twice_and_run_after", kill_twice)

    async def gone_is_not_found():
        from e2b import SandboxException  # noqa: F401
        from e2b.exceptions import NotFoundException

        async def all_404(sb, how):
            for name, call in (
                ("get_info", lambda: sb.get_info()),
                ("set_timeout", lambda: sb.set_timeout(60)),
                ("connect", lambda: AsyncSandbox.connect(sb.sandbox_id)),
            ):
                try:
                    await call()
                except NotFoundException:
                    continue
                except Exception as e:  # noqa: BLE001
                    raise AssertionError(f"{how} {name}: {type(e).__name__}: {e}")
                raise AssertionError(f"{how} {name} succeeded")

        killed = await AsyncSandbox.create(timeout=60)
        await killed.kill()
        await all_404(killed, "killed")
        # set_timeout(0) kills before it answers.
        zero = await AsyncSandbox.create(timeout=60)
        await zero.set_timeout(0)
        await all_404(zero, "set_timeout(0)")
        expired = await AsyncSandbox.create(timeout=1)
        await asyncio.sleep(4)
        await all_404(expired, "expired")

    await check("gone_sandbox_is_404", gone_is_not_found)

    async def template_build_and_use():
        # The Template builder end to end (no steps: the mock guest cannot run
        # shell commands; the real-Space lane builds with RUN and COPY).
        from e2b import AsyncTemplate, Template

        name = "hermetic-" + os.urandom(3).hex()
        logs = []
        info = await AsyncTemplate.build(
            Template().from_base_image(), name, tags=["v1", "stable"], on_build_logs=logs.append
        )
        assert info.name == name and info.template_id and info.build_id, info
        assert any("Build finished" in getattr(lnk, "message", "") for lnk in logs), [
            lnk.message for lnk in logs
        ][-3:]
        assert await AsyncTemplate.exists(name) is True
        assert await AsyncTemplate.exists(name + ":v1") is True
        assert await AsyncTemplate.exists(name + ":nope") is False
        assert await AsyncTemplate.alias_exists(name) is True
        tags = sorted(t.tag for t in await AsyncTemplate.get_tags(info.template_id))
        assert tags == ["stable", "v1"], tags
        await AsyncTemplate.assign_tags(name + ":v1", ["extra"])
        await AsyncTemplate.remove_tags(name, ["stable"])
        tags = sorted(t.tag for t in await AsyncTemplate.get_tags(info.template_id))
        assert tags == ["extra", "v1"], tags
        box = await AsyncSandbox.create(name + ":v1", timeout=60)
        try:
            assert (await box.get_info()).template_id == info.template_id
            assert (await box.commands.run("echo from-template")).stdout == "from-template\n"
        finally:
            await box.kill()
        # A build from a base the gateway cannot boot fails with a build error.
        try:
            await AsyncTemplate.build(Template().from_image("python:3-nope"), name + "-bad")
        except e2b.BuildException as e:
            assert "cannot be booted" in str(e), str(e)
        else:
            raise AssertionError("an unbootable base image built")

    await check("template_build_and_use", template_build_and_use)

    async def many_parallel_sandboxes():
        # Hundreds of sandboxes at once, each used and killed; the gateway's
        # records, reaper and state writes must hold up.
        n = int(os.environ.get("HERMETIC_PARALLEL", "200"))

        async def one(i):
            s = await AsyncSandbox.create(timeout=120, metadata={"batch": "par", "i": str(i)})
            r = await s.commands.run(f"echo {i}")
            assert r.stdout == f"{i}\n", r.stdout
            await s.files.write(f"/tmp/par-{i}.txt", str(i))
            assert await s.files.read(f"/tmp/par-{i}.txt") == str(i)
            return s

        boxes = await asyncio.gather(*[one(i) for i in range(n)])
        try:
            from e2b import SandboxQuery

            p = AsyncSandbox.list(query=SandboxQuery(metadata={"batch": "par"}), limit=50)
            seen = 0
            while True:
                seen += len(await p.next_items())
                if not p.has_next:
                    break
            assert seen == n, (seen, n)
        finally:
            killed = await asyncio.gather(*[b.kill() for b in boxes])
        assert all(killed)

    await check("parallel_sandboxes", many_parallel_sandboxes)

    async def sibling_forks():
        # 32 sandboxes each forked 8 ways at the same time.
        parents = await asyncio.gather(*[AsyncSandbox.create(timeout=120) for _ in range(32)])
        try:
            for i, p in enumerate(parents):
                await p.files.write("/tmp/parent.txt", f"p{i}")
            families = await asyncio.gather(*[p.fork(count=8, timeout=120) for p in parents])
            kids = [k for fam in families for k in fam]
            assert len(kids) == 32 * 8 and all(isinstance(k, AsyncSandbox) for k in kids), [
                k for k in kids if not isinstance(k, AsyncSandbox)
            ][:3]
            got = await asyncio.gather(*[k.files.read("/tmp/parent.txt") for k in kids])
            for i, fam in enumerate(families):
                assert all(g == f"p{i}" for g in got[i * 8 : (i + 1) * 8])
            await asyncio.gather(*[k.kill() for k in kids])
        finally:
            await asyncio.gather(*[p.kill() for p in parents])

    await check("sibling_forks_32x8", sibling_forks)

    async def bad_key():
        try:
            await AsyncSandbox.create(api_key="e2b_" + "0" * 40)
        except Exception as e:  # noqa: BLE001
            assert type(e).__name__ == "AuthenticationException", type(e)
            return
        raise AssertionError("bad key accepted")

    await check("wrong_api_key", bad_key)


print(
    "e2b",
    getattr(e2b, "__version__", None) or __import__("importlib.metadata").metadata.version("e2b"),
)
asyncio.run(main())
