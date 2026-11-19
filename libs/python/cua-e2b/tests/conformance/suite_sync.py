"""The sync half of the conformance suite: the areas where `e2b.Sandbox`
differs from `AsyncSandbox` (the iteration protocol of CommandHandle,
`wait(on_stdout=...)`, `WatchHandle.get_new_events`, streamed reads as a
plain iterator, the paginator, ptys through `wait(on_pty=...)`).

Cases are recorded as `sync.<name>` in the same results file as the async
ones. Each takes the sync sandbox `ssb` and runs in a worker thread.
"""

from __future__ import annotations

import re
import time

import e2b
from e2b import Sandbox

from .common import (
    SHARED,
    exc_info,
    host_shape,
    result_info,
    sha,
    sprobe,
    sync_case,
)


def _until(pred, timeout=10.0, step=0.2):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        time.sleep(step)
    return bool(pred())


@sync_case
def run_basic(ssb):
    return {
        "fg": result_info(ssb.commands.run("echo hi")),
        "nonzero": sprobe(lambda: ssb.commands.run("echo o; echo e >&2; exit 3")),
        "cwd_envs": ssb.commands.run('echo "$X:$PWD"', envs={"X": "1"}, cwd="/tmp").stdout,
        "timeout": sprobe(lambda: ssb.commands.run("sleep 10", timeout=2)),
        "missing_cwd": sprobe(
            lambda: ssb.commands.run("pwd", cwd="/no/such/dir-conf"), result_info
        ),
        "signal": sprobe(lambda: ssb.commands.run("kill -9 $$"), result_info),
        "kill_unknown": ssb.commands.kill(999999),
    }


@sync_case
def handle_iteration(ssb):
    h = ssb.commands.run(
        "echo a; sleep 0.4; echo b >&2; sleep 0.4; echo c", background=True, timeout=0
    )
    seen = []
    for stdout, stderr, pty in h:
        seen.append([stdout, stderr, pty])
    return {
        "handle_type": type(h).__name__,
        "chunks": seen,
        "result": result_info(h.wait()),
        "iterate_again": [list(x) for x in h],
    }


@sync_case
def wait_callbacks(ssb):
    out, err = [], []
    h = ssb.commands.run(
        "echo o1; sleep 0.3; echo e1 >&2; sleep 0.3; echo o2", background=True, timeout=0
    )
    r = h.wait(on_stdout=out.append, on_stderr=err.append)
    return {"stdout_cb": "".join(out), "stderr_cb": "".join(err), "result": result_info(r)}


@sync_case
def handle_state(ssb):
    h = ssb.commands.run("echo one; sleep 2; echo two", background=True, timeout=0)
    time.sleep(1.0)
    live_attrs = {a: hasattr(h, a) for a in ("stdout", "stderr", "exit_code", "error")}
    r = h.wait()
    return {
        "live_attrs": live_attrs,
        "result": result_info(r),
        "wait_twice_equal": result_info(h.wait()) == result_info(r),
        "kill_exited": h.kill(),
    }


@sync_case
def commands_misc(ssb):
    h = ssb.commands.run(
        "for i in 1 2 3 4; do echo l$i; sleep 0.5; done", background=True, timeout=0
    )
    time.sleep(1.2)
    seen = []
    c = ssb.commands.connect(h.pid)
    r = c.wait(on_stdout=seen.append)
    s = ssb.commands.run("cat > /tmp/conf-sync-stdin.txt", background=True, stdin=True, timeout=30)
    s.send_stdin("abc\n")
    ssb.commands.send_stdin(s.pid, b"def\n")
    ssb.commands.close_stdin(s.pid)
    s.wait()
    procs = ssb.commands.list()
    sleeper = ssb.commands.run("sleep 31", background=True, timeout=0, cwd="/tmp")
    listed = [p for p in ssb.commands.list() if p.pid == sleeper.pid]
    killed = sleeper.kill()
    return {
        "connect_type": type(c).__name__,
        "connect_new_only": "l1\n" not in "".join(seen),
        "connect_end": r.stdout.endswith("l4\n"),
        "stdin_file": ssb.files.read("/tmp/conf-sync-stdin.txt"),
        "list_type": type(procs).__name__,
        "listed": [[p.cmd, p.args, p.cwd] for p in listed],
        "killed": killed,
        "wait_after_kill": sprobe(sleeper.wait, result_info),
        "connect_unknown": sprobe(lambda: ssb.commands.connect(999999).wait(), result_info),
        "send_stdin_unknown": sprobe(lambda: ssb.commands.send_stdin(999999, "x")),
        "close_stdin_unknown": sprobe(lambda: ssb.commands.close_stdin(999999)),
    }


@sync_case
def pty_wait_on_pty(ssb):
    got = []
    h = ssb.pty.create(e2b.PtySize(rows=24, cols=80), timeout=60)
    ssb.pty.send_stdin(h.pid, b"echo SZ[$(stty size)] sync-pty-$((20+1)); exit 5\n")
    r = sprobe(lambda: h.wait(on_pty=got.append), result_info)
    text = b"".join(got).decode(errors="replace")
    m = re.search(r"SZ\[(\d+ \d+)\]", text)
    h2 = ssb.pty.create(e2b.PtySize(rows=24, cols=80), timeout=60)
    ssb.pty.resize(h2.pid, e2b.PtySize(rows=30, cols=100))
    killed = ssb.pty.kill(h2.pid)
    return {
        "chunk_type": type(got[0]).__name__ if got else None,
        "size": m.group(1) if m else None,
        "saw_output": "sync-pty-21" in text,
        "exit": r,
        "kill": killed,
        "kill_unknown": ssb.pty.kill(999999),
        "wait_after_kill": sprobe(h2.wait, result_info),
    }


@sync_case
def stream_input(ssb):
    if not hasattr(ssb.commands, "stream_input"):
        return {"since": "2.58"}
    path = "/tmp/conf-sync-stream-input.txt"
    h = ssb.commands.run(f"cat > {path}", background=True, stdin=True, timeout=60)
    sent = sprobe(lambda: ssb.commands.stream_input(h.pid, ["a\n", b"b\n", "c\n"]))
    ssb.commands.close_stdin(h.pid)
    h.wait()
    return {
        "send": sent,
        "file": ssb.files.read(path),
        "single_str": sprobe(lambda: ssb.commands.stream_input(h.pid, "abc")),
        "single_bytes": sprobe(lambda: ssb.commands.stream_input(h.pid, b"abc")),
        "generator": sprobe(lambda: ssb.commands.stream_input(999999, (x for x in ["x"]))),
        "unknown_pid": sprobe(lambda: ssb.commands.stream_input(999999, ["x"])),
        "pty_unknown_pid": (
            sprobe(lambda: ssb.pty.stream_input(999999, ["x"]))
            if hasattr(ssb.pty, "stream_input")
            else "n/a"
        ),
    }


@sync_case
def files_basic(ssb):
    p = "/tmp/conf-sync/a.txt"
    w = ssb.files.write(p, "héllo")
    data = bytes(range(256))
    ssb.files.write("/tmp/conf-sync/all.bin", data)
    return {
        "write": [type(w).__name__, w.name, w.path],
        "text": ssb.files.read(p),
        "bytes_equal": bytes(ssb.files.read("/tmp/conf-sync/all.bin", format="bytes")) == data,
        "info": [ssb.files.get_info(p).size, str(ssb.files.get_info(p).type)],
        "exists": [ssb.files.exists(p), ssb.files.exists("/tmp/conf-sync/nope")],
        "list": sorted(e.name for e in ssb.files.list("/tmp/conf-sync")),
        "make_dir": [
            ssb.files.make_dir("/tmp/conf-sync/d"),
            ssb.files.make_dir("/tmp/conf-sync/d"),
        ],
        "rename": ssb.files.rename(p, "/tmp/conf-sync/b.txt").path,
        "remove_missing": ssb.files.remove("/tmp/conf-sync/nope"),
        "write_files": [
            i.name
            for i in ssb.files.write_files(
                [
                    {"path": "/tmp/conf-sync/w1.txt", "data": "1"},
                    {"path": "/tmp/conf-sync/w2.txt", "data": b"2"},
                ]
            )
        ],
        "read_missing": sprobe(lambda: ssb.files.read("/tmp/conf-nope-404")),
        "list_depth_neg": sprobe(lambda: ssb.files.list("/tmp", depth=-1)),
        "list_missing": sprobe(lambda: ssb.files.list("/tmp/conf-nope-404")),
        "get_info_missing": sprobe(lambda: ssb.files.get_info("/tmp/conf-nope-404")),
        "make_dir_over_file": sprobe(lambda: ssb.files.make_dir("/tmp/conf-sync/b.txt")),
    }


@sync_case
def files_read_stream(ssb):
    p = "/tmp/conf-sync/stream.bin"
    data = bytes(range(256)) * (3 * 4096)
    ssb.files.write(p, data)
    res = {}
    s = ssb.files.read(p, format="stream")
    res["iterator"] = hasattr(s, "__iter__")
    res["has_context_manager"] = hasattr(s, "__enter__")
    got, n = b"", 0
    for c in s:
        got += c
        n += 1
        res["chunk_type"] = type(c).__name__
    res["reassembled"] = sha(got) == sha(data)
    res["chunked"] = n > 1
    s2 = ssb.files.read(p, format="stream")
    first = next(iter(s2))
    res["early_first"] = bool(first)
    res["close"] = sprobe(lambda: s2.close()) if hasattr(s2, "close") else "n/a"
    res["close_twice"] = sprobe(lambda: s2.close()) if hasattr(s2, "close") else "n/a"
    if hasattr(s, "__enter__"):
        with ssb.files.read(p, format="stream") as s3:
            res["with_total"] = sum(len(c) for c in s3) == len(data)
    res["stream_missing"] = sprobe(
        lambda: list(ssb.files.read("/tmp/conf-nope-404", format="stream"))
    )
    return res


@sync_case
def watch_get_new_events(ssb):
    d = "/tmp/conf-sync-watch"
    ssb.commands.run(f"rm -rf {d}; mkdir -p {d}/sub")
    w = ssb.files.watch_dir(d)
    time.sleep(1.0)
    res = {"handle_type": type(w).__name__}
    ssb.commands.run(f"touch {d}/a.txt; mkdir {d}/newdir")
    time.sleep(1.5)
    ev = w.get_new_events()
    res["first"] = sorted({f"{e.type.value}:{e.name}" for e in ev})
    res["event_class"] = type(ev[0]).__name__ if ev else None
    res["second_is_empty"] = w.get_new_events() == []
    ssb.commands.run(f"touch {d}/b.txt")
    time.sleep(1.2)
    w.stop()
    res["after_stop"] = sprobe(lambda: sorted(e.name for e in w.get_new_events()))
    res["stop_twice"] = sprobe(w.stop)
    rec = ssb.files.watch_dir(d, recursive=True)
    time.sleep(1.0)
    ssb.commands.run(f"touch {d}/sub/deep.txt")
    time.sleep(1.5)
    res["recursive"] = sorted({f"{e.type.value}:{e.name}" for e in rec.get_new_events()})
    rec.stop()
    res["missing"] = sprobe(lambda: ssb.files.watch_dir("/tmp/conf-nope-404"))
    res["on_file"] = sprobe(lambda: ssb.files.watch_dir("/etc/hostname"))
    return res


@sync_case
def list_paginator(ssb):
    rid = SHARED["run_id"]
    Q = e2b.SandboxQuery
    q = Q(metadata={"sync_grp": rid})
    p = Sandbox.list(query=q, limit=1)
    pages = []
    while p.has_next and len(pages) < 10:
        pages.append([i.sandbox_id for i in p.next_items()])
    return {
        "paginator": type(p).__name__,
        "pages": [len(x) for x in pages],
        "finds_sync_sandbox": sum(pages, []) == [ssb.sandbox_id],
        "has_next_end": p.has_next,
        "next_token_end": p.next_token,
        "next_after_end": sprobe(p.next_items, lambda xs: len(xs)),
        "default_list": type(Sandbox.list().next_items()).__name__,
        "state_filter_running": len(
            Sandbox.list(
                query=Q(metadata={"sync_grp": rid}, state=[e2b.SandboxState.RUNNING])
            ).next_items()
        ),
        "state_filter_paused": len(
            Sandbox.list(
                query=Q(metadata={"sync_grp": rid}, state=[e2b.SandboxState.PAUSED])
            ).next_items()
        ),
        "info_metadata": {
            k: ("<run>" if k == "sync_grp" else v) for k, v in ssb.get_info().metadata.items()
        },
    }


@sync_case
def sandbox_class_api(ssb):
    uid = "iconf0000000000000000"
    other = Sandbox.connect(ssb.sandbox_id)
    return {
        "connect_type": type(other).__name__,
        "connect_runs": other.commands.run("echo c").stdout,
        "get_info_class": [type(Sandbox.get_info(ssb.sandbox_id)).__name__],
        "set_timeout_class": Sandbox.set_timeout(ssb.sandbox_id, 1500),
        "metrics_class": type(Sandbox.get_metrics(ssb.sandbox_id)).__name__,
        "is_running": ssb.is_running(),
        "get_host": host_shape(ssb.get_host(8080), ssb.sandbox_id),
        "kill_unknown": sprobe(lambda: Sandbox.kill(uid)),
        "connect_unknown": sprobe(lambda: Sandbox.connect(uid)),
        "get_info_unknown": sprobe(lambda: Sandbox.get_info(uid)),
        "upload_url_keys": sorted(
            __import__("urllib.parse").parse.parse_qs(
                __import__("urllib.parse").parse.urlsplit(ssb.upload_url("/tmp/x")).query
            )
        ),
    }


@sync_case
def context_manager(ssb):
    from .common import SHARED as S

    template = S.get("template")
    with Sandbox.create(
        template=template, timeout=120, metadata={"suite": "e2b", "case": "sync-ctx"}
    ) as sb:
        sid = sb.sandbox_id
        out = {"enter_returns_self": True, "runs": sb.commands.run("echo in").stdout}
    out["after_exit_info"] = sprobe(lambda: Sandbox.get_info(sid), lambda i: str(i.state))
    out["after_exit_kill"] = sprobe(lambda: Sandbox.kill(sid))
    out["run_after_exit"] = sprobe(
        lambda: sb.commands.run("echo x", request_timeout=20), result_info
    )
    return out
