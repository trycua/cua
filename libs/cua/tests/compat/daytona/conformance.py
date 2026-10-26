"""Daytona SDK conformance on real Spaces (a real shell in the guest).

The unmodified `daytona` SDK against `cua compat serve --provider daytona`,
asserting the behaviour the SDK documents: exec, code run, sessions (cwd and
exports carry over, async commands, logs, follow), files (upload, download,
list, info, folder, move, permissions, find, search, replace, bulk), preview
links, lifecycle (stop/start, labels, intervals), snapshot and fork.

There is no Daytona account to record fixtures from (see compat/daytona-map.md),
so this suite asserts instead of diffing. Prints PASS/FAIL per check and a
JSON summary line; exits 1 on any failure.
"""

from __future__ import annotations

import json
import os
import ssl
import sys
import time
import traceback
import urllib.request

from daytona import (
    CreateSandboxFromSnapshotParams,
    Daytona,
    DaytonaConfig,
    FileUpload,
    SessionExecuteRequest,
)

failures: list[str] = []
passed: list[str] = []


def check(name, fn):
    try:
        fn()
        passed.append(name)
        print(f"PASS {name}", file=sys.stderr)
    except BaseException as e:  # noqa: BLE001
        failures.append(name)
        print(f"FAIL {name}: {type(e).__name__}: {e}", file=sys.stderr)
        traceback.print_exc()


def fetch(url, headers=None):
    ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
    req = urllib.request.Request(url, headers=headers or {})
    last = None
    for _ in range(20):
        try:
            with urllib.request.urlopen(req, timeout=10, context=ctx) as r:
                return r.status, r.read().decode()
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode()
        except Exception as e:  # noqa: BLE001
            last = e
            time.sleep(1)
    raise AssertionError(f"fetch {url}: {last}")


def main():
    d = Daytona(DaytonaConfig())
    sb = d.create(
        CreateSandboxFromSnapshotParams(labels={"suite": "conformance"}, auto_stop_interval=0)
    )
    made = [sb]
    try:
        suite(d, sb, made)
    finally:
        for s in made:
            try:
                d.delete(s)
            except Exception:  # noqa: BLE001
                pass
    print(json.dumps({"passed": passed, "failed": failures}))
    sys.exit(1 if failures else 0)


def suite(d: Daytona, sb, made):
    def exec_basic():
        r = sb.process.exec("echo hi; echo err >&2; exit 0")
        assert r.exit_code == 0 and "hi" in r.result and "err" in r.result, (r.exit_code, r.result)
        r = sb.process.exec("exit 7")
        assert r.exit_code == 7, r.exit_code
        r = sb.process.exec("pwd", cwd="/tmp")
        assert r.result.strip() == "/tmp", r.result
        r = sb.process.exec('echo "$FOO"', env={"FOO": "bar"})
        assert r.result.strip() == "bar", r.result
        assert sb.process.exec("whoami").result.strip() == "daytona"

    check("exec", exec_basic)

    def code_run():
        from daytona import CodeRunParams

        r = sb.process.code_run(
            "import sys; print(6*7, sys.argv[1:])", params=CodeRunParams(argv=["a"])
        )
        assert r.exit_code == 0 and r.result.strip() == "42 ['a']", (r.exit_code, r.result)

    check("code_run", code_run)

    def sessions():
        sb.process.create_session("s1")
        r = sb.process.execute_session_command(
            "s1", SessionExecuteRequest(command="cd /tmp && export X=7")
        )
        assert r.exit_code == 0, r.exit_code
        r = sb.process.execute_session_command(
            "s1", SessionExecuteRequest(command='echo "$PWD:$X"')
        )
        assert r.stdout.strip() == "/tmp:7", r.stdout
        a = sb.process.execute_session_command(
            "s1", SessionExecuteRequest(command="sleep 1; echo late; echo bad >&2", run_async=True)
        )
        assert a.cmd_id
        for _ in range(50):
            c = sb.process.get_session_command("s1", a.cmd_id)
            if c.exit_code is not None:
                break
            time.sleep(0.2)
        assert c.exit_code == 0, c
        logs = sb.process.get_session_command_logs("s1", a.cmd_id)
        assert logs.stdout.strip() == "late" and logs.stderr.strip() == "bad", logs
        s = sb.process.get_session("s1")
        assert len(s.commands) == 3, s.commands
        assert any(x.session_id == "s1" for x in sb.process.list_sessions())
        sb.process.delete_session("s1")

    check("sessions", sessions)

    def follow_logs():
        import asyncio

        sb.process.create_session("s2")
        a = sb.process.execute_session_command(
            "s2",
            SessionExecuteRequest(
                command="for i in 1 2 3; do echo o$i; sleep 0.3; done; echo e >&2", run_async=True
            ),
        )
        loop_out = asyncio.run(_follow(sb, "s2", a.cmd_id))
        assert "o1" in loop_out[0] and "o3" in loop_out[0] and "e" in loop_out[1], loop_out
        sb.process.delete_session("s2")

    check("session_logs_follow", follow_logs)

    def files():
        sb.fs.upload_file(b"hello world", "/tmp/f/a.txt")
        assert sb.fs.download_file("/tmp/f/a.txt") == b"hello world"
        sb.fs.create_folder("/tmp/f/sub", "700")
        info = sb.fs.get_file_info("/tmp/f/sub")
        assert info.is_dir and info.permissions.endswith("700"), info
        names = sorted(f.name for f in sb.fs.list_files("/tmp/f"))
        assert names == ["a.txt", "sub"], names
        sb.fs.set_file_permissions("/tmp/f/a.txt", mode="600")
        assert sb.fs.get_file_info("/tmp/f/a.txt").permissions.endswith("600")
        found = sb.fs.find_files("/tmp/f", "world")
        assert found and found[0].file.endswith("a.txt") and found[0].line == 1, found
        res = sb.fs.search_files("/tmp/f", "*.txt")
        assert any(f.endswith("a.txt") for f in res.files), res
        rep = sb.fs.replace_in_files(["/tmp/f/a.txt"], "world", "cua")
        assert rep[0].success, rep
        assert sb.fs.download_file("/tmp/f/a.txt") == b"hello cua"
        sb.fs.move_files("/tmp/f/a.txt", "/tmp/f/b.txt")
        sb.fs.upload_files(
            [
                FileUpload(source=b"1", destination="rel/1.txt"),
                FileUpload(source=b"2", destination="/tmp/f/2.txt"),
            ]
        )
        assert sb.fs.download_file("rel/1.txt") == b"1"
        sb.fs.delete_file("/tmp/f", recursive=True)

    check("files", files)

    def pty():
        from daytona import PtySize

        h = sb.process.create_pty_session(id="p1", pty_size=PtySize(cols=80, rows=24))
        h.send_input("echo pty-$((6*7))\nexit\n")
        out = []
        r = h.wait(on_data=out.append, timeout=30)
        assert b"pty-42" in b"".join(out), out
        assert r.exit_code == 0, r

    check("pty", pty)

    def preview():
        sb.process.exec("mkdir -p /tmp/www && echo -n hello-preview > /tmp/www/index.html")
        sb.process.exec("cd /tmp/www && nohup python3 -m http.server 8080 > /tmp/www.log 2>&1 &")
        link = sb.get_preview_link(8080)
        status, body = fetch(link.url, {"x-daytona-preview-token": link.token})
        assert (status, body) == (200, "hello-preview"), (status, body)

    check("preview", preview)

    def lifecycle():
        sb.set_labels({"suite": "conformance", "x": "1"})
        sb.set_autostop_interval(60)
        got = d.get(sb.id)
        assert got.labels.get("x") == "1" and got.auto_stop_interval == 60
        sb.process.exec("echo kept > /tmp/kept.txt")
        sb.stop()
        assert str(d.get(sb.id).state).lower().endswith("stopped")
        sb.start()
        assert sb.process.exec("cat /tmp/kept.txt").result.strip() == "kept"

    check("stop_start_labels", lifecycle)

    def snapshot_fork():
        sb.process.exec("echo world > /tmp/world.txt")
        name = f"cua-conf-{int(time.time())}"
        sb.create_snapshot(name)
        child = d.create(CreateSandboxFromSnapshotParams(snapshot=name))
        made.append(child)
        assert child.process.exec("cat /tmp/world.txt").result.strip() == "world"
        kid = sb.fork()
        made.append(kid)
        assert kid.process.exec("cat /tmp/world.txt").result.strip() == "world"
        d.delete(child)
        made.remove(child)
        d.snapshot.delete(d.snapshot.get(name))

    check("snapshot_and_fork", snapshot_fork)


async def _follow(sb, sid, cid):
    from daytona import AsyncDaytona

    out, err = [], []
    async with AsyncDaytona() as ad:
        asb = await ad.get(sb.id)
        await asb.process.get_session_command_logs_async(sid, cid, out.append, err.append)
    return "".join(out), "".join(err)


main()
