"""Conformance cases: commands and ptys (async API)."""

from __future__ import annotations

import asyncio
import gc
import re
import shlex

import e2b

from .common import (
    ROOT,
    case,
    exc_info,
    probe,
    result_info,
    sha,
    until,
)

# the five cases below share this stream shape: a command that prints a
# known amount of text, so the length and hash say whether every byte came


def _lines(s: str) -> list:
    return [x for x in s.split("\n") if x]


@case
async def cmd_interleaved_order(sb):
    seq = []
    r = await sb.commands.run(
        "for i in 1 2 3; do echo o$i; sleep 0.3; echo e$i >&2; sleep 0.3; done",
        on_stdout=lambda s: seq.append(["out", s]),
        on_stderr=lambda s: seq.append(["err", s]),
    )
    return {"seq": seq, "stdout": r.stdout, "stderr": r.stderr}


async def _big(sb, nbytes: int) -> dict:
    chunks = []
    r = await sb.commands.run(
        f"yes abcdefghijklmnopqrstuvwxyz | head -c {nbytes}",
        on_stdout=chunks.append,
        timeout=120,
        request_timeout=120,
    )
    joined = "".join(chunks)
    return {
        "len": len(r.stdout),
        "sha": sha(r.stdout.encode()),
        "callback_equal": joined == r.stdout,
        "stderr": r.stderr,
        "exit_code": r.exit_code,
    }


@case
async def cmd_output_1mb(sb):
    return await _big(sb, 1024 * 1024)


@case
async def cmd_output_10mb(sb):
    return await _big(sb, 10 * 1024 * 1024)


@case
async def cmd_output_binary(sb):
    chunks = []
    r = await sb.commands.run(r"printf 'a\377\376\000b\200c'", on_stdout=chunks.append)
    return {"stdout": r.stdout, "chunks": chunks, "stderr": r.stderr}


@case
async def cmd_output_multibyte_split(sb):
    py = (
        "import sys,time\nb='é✓'.encode()\n"
        "for i in range(len(b)):\n"
        "    sys.stdout.buffer.write(b[i:i+1]); sys.stdout.buffer.flush(); time.sleep(0.15)\n"
    )
    chunks = []
    r = await sb.commands.run(f"python3 -c {shlex.quote(py)}", on_stdout=chunks.append)
    return {"stdout": r.stdout, "chunks": chunks}


@case
async def cmd_exit_codes(sb):
    out = {}
    for label, cmd in (
        ("exit_255", "exit 255"),
        ("exit_256", "exit 256"),
        ("exit_137", "exit 137"),
        ("exit_neg", "exit -1"),
        ("sigterm_self", "kill -TERM $$"),
        ("sigkill_self", "kill -9 $$"),
        ("sigsegv_self", "kill -SEGV $$"),
        ("sigterm_child", "sleep 30 & kill -TERM $!; wait $!"),
        ("not_found", "no-such-command-conf"),
        ("not_executable", "/etc/passwd"),
    ):
        out[label] = await probe(sb.commands.run(cmd), result_info)
    return out


@case
async def cmd_envs_precedence(sb):
    # the shared sandbox is created with CONF_SB and CONF_BOTH (suite.run_suite)
    probe_cmd = (
        'printf "%s|%s|%s" "$CONF_SB" "$CONF_BOTH" "$CONF_CALL"; '
        '[ -z "${CONF_EMPTY+x}" ] && printf "|unset" || printf "|set:[%s]" "$CONF_EMPTY"'
    )
    out = {}
    out["sandbox_only"] = (await sb.commands.run(probe_cmd)).stdout
    out["call_overrides"] = (
        await sb.commands.run(
            probe_cmd, envs={"CONF_BOTH": "call", "CONF_CALL": "c", "CONF_EMPTY": ""}
        )
    ).stdout
    for label, value in (
        ("unicode", "héllo ✓ 日本"),
        ("quotes", "a\"b'c$d`e`\\f"),
        ("newlines", "line1\nline2\n"),
        ("leading_space", "  padded  "),
        ("equals", "k=v=w"),
    ):
        out[label] = (
            await sb.commands.run('printf %s "$CONF_V"', envs={"CONF_V": value})
        ).stdout == value
    out["bad_name"] = await probe(
        sb.commands.run("printf %s ok", envs={"BAD=NAME": "x"}), result_info
    )
    return out


@case
async def cmd_cwd_and_user_errors(sb):
    return {
        "missing_cwd": await probe(sb.commands.run("pwd", cwd="/no/such/dir-conf"), result_info),
        "cwd_is_file": await probe(sb.commands.run("pwd", cwd="/etc/passwd"), result_info),
        "missing_user": await probe(sb.commands.run("id -u", user="nosuchuser"), result_info),
        "empty_command": await probe(sb.commands.run(""), result_info),
        "only_spaces": await probe(sb.commands.run("   "), result_info),
        "relative_cwd": await probe(
            sb.commands.run("pwd", cwd="tmp"), lambda r: r.stdout.strip().endswith("/tmp")
        ),
    }


@case
async def cmd_timeouts(sb):
    out = {}
    out["timeout_1"] = await probe(
        sb.commands.run("echo partial; echo err >&2; sleep 2023", timeout=1), result_info
    )
    await asyncio.sleep(2)
    r = await sb.commands.run("pgrep -f '[s]leep 2023' >/dev/null && echo y || echo n")
    out["survived_after_timeout"] = r.stdout.strip()
    await sb.commands.run("pkill -f '[s]leep 2023'; true")
    out["timeout_0"] = await probe(sb.commands.run("sleep 2; echo ok", timeout=0), result_info)
    out["timeout_float"] = await probe(sb.commands.run("sleep 30", timeout=0.5), result_info)
    return out


@case
async def cmd_wait_twice(sb):
    ok = await sb.commands.run("echo a", background=True)
    r1 = await ok.wait()
    r2 = await ok.wait()
    bad = await sb.commands.run("echo b; exit 2", background=True)
    first = await probe(bad.wait(), result_info)
    second = await probe(bad.wait(), result_info)
    return {
        "same_object": r1 is r2,
        "equal": result_info(r1) == result_info(r2),
        "failing_first": first,
        "failing_second": second,
        "exit_code_after": bad.exit_code,
        "error_after": bad.error,
    }


@case
async def cmd_handle_live(sb):
    h = await sb.commands.run("echo one; sleep 3; echo two; echo err >&2", background=True)
    await asyncio.sleep(1.5)
    live = {"stdout": h.stdout, "stderr": h.stderr, "exit_code": h.exit_code, "error": h.error}
    r = await h.wait()
    return {
        "live": live,
        "after": {"stdout": h.stdout, "stderr": h.stderr, "exit_code": h.exit_code},
        "result_is_handle_state": r.stdout == h.stdout and r.stderr == h.stderr,
    }


@case
async def cmd_connect_running(sb):
    h = await sb.commands.run(
        "for i in 1 2 3 4 5 6; do echo line$i; sleep 0.6; done", background=True, timeout=0
    )
    await asyncio.sleep(1.7)
    before = _lines(h.stdout)
    seen = []
    c = await sb.commands.connect(h.pid, on_stdout=seen.append)
    res = await c.wait()
    orig = await h.wait()
    seen_lines = _lines("".join(seen))
    out = {
        "handle_type": type(c).__name__,
        "same_pid": c.pid == h.pid,
        "new_output_only": not set(before[:-1]) & set(seen_lines),
        "reaches_end": bool(seen_lines) and seen_lines[-1] == "line6",
        "result_stdout_equals_callback": res.stdout == "".join(seen),
        "exit_code": res.exit_code,
        "original_full": _lines(orig.stdout) == [f"line{i}" for i in range(1, 7)],
    }
    # disconnect, then reconnect later
    h2 = await sb.commands.run("sleep 3; echo end", background=True, timeout=0)
    await h2.disconnect()
    c2 = await sb.commands.connect(h2.pid)
    r2 = await c2.wait()
    out["reconnect"] = result_info(r2)
    return out


@case
async def cmd_unknown_and_exited_pids(sb):
    async def _connect_wait(pid):
        h = await sb.commands.connect(pid)
        return await h.wait()

    done = await sb.commands.run("true", background=True)
    await done.wait()
    return {
        "kill_exited": await probe(sb.commands.kill(done.pid)),
        "kill_unknown": await probe(sb.commands.kill(999999)),
        "connect_unknown": await probe(_connect_wait(999999), result_info),
        "send_stdin_unknown": await probe(sb.commands.send_stdin(999999, "x")),
        "close_stdin_unknown": await probe(sb.commands.close_stdin(999999)),
        "send_stdin_exited": await probe(sb.commands.send_stdin(done.pid, "x")),
        "connect_exited": await probe(_connect_wait(done.pid), result_info),
    }


@case
async def cmd_concurrent_20(sb):
    t = asyncio.get_running_loop().time()
    rs = await asyncio.gather(
        *[sb.commands.run(f"echo n{i}; sleep 1", timeout=60) for i in range(20)],
        return_exceptions=True,
    )
    took = asyncio.get_running_loop().time() - t
    ok = [r for r in rs if not isinstance(r, BaseException)]
    return {
        "succeeded": len(ok),
        "outputs_match": sorted(r.stdout for r in ok) == sorted(f"n{i}\n" for i in range(20)),
        "ran_in_parallel": took < 15,
        "errors": sorted({type(r).__name__ for r in rs if isinstance(r, BaseException)}),
    }


async def _feed(sb, data, chunks=None):
    path = "/tmp/conf-stdin.bin"
    h = await sb.commands.run(f"cat > {path}", background=True, stdin=True, timeout=120)
    for c in chunks or [data]:
        await h.send_stdin(c)
    await sb.commands.close_stdin(h.pid)
    await h.wait()
    return bytes(await sb.files.read(path, format="bytes"))


@case
async def cmd_stdin_variants(sb):
    allbytes = bytes(range(256))
    mb = ("0123456789abcdef" * 64) * 1024
    out = {}
    out["binary_256"] = await probe(_feed(sb, allbytes), lambda b: b == allbytes)
    out["str_unicode"] = await probe(_feed(sb, "héllo ✓\n"), lambda b: b.decode())
    out["bytes_unicode"] = await probe(_feed(sb, "héllo ✓\n".encode()), lambda b: b.decode())
    out["one_mb_one_send"] = await probe(_feed(sb, mb), lambda b: sha(b) == sha(mb.encode()))
    out["one_mb_chunks"] = await probe(
        _feed(sb, mb, chunks=[mb[i : i + 65536] for i in range(0, len(mb), 65536)]),
        lambda b: sha(b) == sha(mb.encode()),
    )
    h = await sb.commands.run("cat", background=True, timeout=20)
    out["no_stdin_flag"] = await probe(h.send_stdin("x\n"))
    await h.kill()
    s = await sb.commands.run("cat", background=True, stdin=True, timeout=20)
    out["close_twice"] = [
        await probe(sb.commands.close_stdin(s.pid)),
        await probe(sb.commands.close_stdin(s.pid)),
    ]
    out["send_after_close"] = await probe(s.send_stdin("late"))
    await probe(s.wait())
    return out


@case
async def cmd_background_survives_handle_gc(sb):
    path = "/tmp/conf-gc.txt"
    await sb.commands.run(f"rm -f {path}")
    h = await sb.commands.run(f"sleep 2; echo gc > {path}", background=True, timeout=0)
    del h
    gc.collect()
    await asyncio.sleep(4)
    return (await sb.commands.run(f"cat {path}")).stdout


@case
async def cmd_list_fields(sb):
    h = await sb.commands.run(
        "sleep 31", background=True, timeout=0, envs={"CONF_A": "1"}, cwd="/tmp"
    )
    try:
        procs = [p for p in await sb.commands.list() if p.pid == h.pid]
        if not procs:
            return {"found": False}
        p = procs[0]
        return {
            "found": True,
            "type": type(p).__name__,
            "fields": sorted(vars(p)),
            "cmd": p.cmd,
            "args": p.args,
            "cwd": p.cwd,
            "tag": p.tag,
            "env_is_dict": isinstance(p.envs, dict),
            "env_has_call_env": p.envs.get("CONF_A") if isinstance(p.envs, dict) else None,
            "pid_is_int": isinstance(p.pid, int),
        }
    finally:
        await h.kill()


@case
async def cmd_tag(sb):
    # `tag` only exists in newer SDKs
    import inspect

    if "tag" not in inspect.signature(sb.commands.run).parameters:
        return {"since": "2.56"}
    h = await sb.commands.run("sleep 20", background=True, timeout=0, tag="conf-tag")
    try:
        mine = [p for p in await sb.commands.list() if p.pid == h.pid]
        return {"listed_tag": mine[0].tag if mine else None}
    finally:
        await h.kill()


# ── pty ────────────────────────────────────────────────────────────────────


def _field(text: str, key: str):
    """`KEY[value]` as printed by the shell (the echoed command line holds
    `KEY[$(...)]`, which does not match the digits/word pattern)."""
    m = re.search(rf"{key}\[([^\]$()]*)\]", text)
    return m.group(1) if m else None


async def _text(out) -> str:
    return b"".join(out).decode(errors="replace")


async def _wait_for(out, needle: str, timeout=10.0) -> bool:
    return await until(lambda: needle in b"".join(out).decode(errors="replace"), timeout)


@case
async def cmd_pty_session(sb):
    out = []
    h = await sb.pty.create(
        e2b.PtySize(rows=24, cols=80),
        on_data=out.append,
        envs={"CONF_P": "1"},
        cwd="/tmp",
    )
    res = {"handle_type": type(h).__name__, "pid_is_int": isinstance(h.pid, int)}
    await sb.pty.send_stdin(h.pid, b"echo SZ[$(stty size)] TERM[$TERM] P[$CONF_P] D[$(pwd)]\n")
    await _wait_for(out, "D[/")
    text = await _text(out)
    res["initial_size"] = _field(text, "SZ")
    res["term"] = _field(text, "TERM")
    res["env"] = _field(text, "P")
    res["cwd"] = _field(text, "D")
    res["chunk_type"] = type(out[0]).__name__ if out else None
    await sb.pty.resize(h.pid, e2b.PtySize(rows=40, cols=120))
    mark = len(out)
    await sb.pty.send_stdin(h.pid, b"echo SZ[$(stty size)]\n")
    await _wait_for(out[mark:], "SZ[4")
    res["resized"] = _field(await _text(out[mark:]), "SZ")
    procs = [p for p in await sb.commands.list() if p.pid == h.pid]
    res["listed"] = (
        {"cmd": procs[0].cmd, "args": procs[0].args, "cwd": procs[0].cwd} if procs else None
    )
    # connect: only output from now on
    out2 = []
    c = await sb.pty.connect(h.pid, on_data=out2.append)
    await sb.pty.send_stdin(h.pid, b"echo second-$((1+1))\n")
    await _wait_for(out2, "second-2")
    res["connect_sees_new"] = "second-2" in await _text(out2)
    res["connect_skips_old"] = "SZ[" not in await _text(out2)
    await c.disconnect()
    await sb.pty.send_stdin(h.pid, b"exit 3\n")
    res["exit"] = await probe(h.wait(), result_info)
    res["kill_exited"] = await probe(sb.pty.kill(h.pid))
    res["kill_unknown"] = await probe(sb.pty.kill(999999))
    res["send_unknown"] = await probe(sb.pty.send_stdin(999999, b"x"))
    res["resize_unknown"] = await probe(sb.pty.resize(999999, e2b.PtySize(rows=10, cols=10)))
    return res


@case
async def cmd_pty_kill(sb):
    out = []
    h = await sb.pty.create(e2b.PtySize(rows=24, cols=80), on_data=out.append, timeout=0)
    await _wait_for(out, "$", 5)
    killed = await sb.pty.kill(h.pid)
    res = await probe(h.wait(), result_info)
    return {"kill": killed, "wait": res}


@case
async def cmd_pty_sizes(sb):
    out = {}
    for label, size in (("tiny", (1, 1)), ("wide", (50, 300))):
        o = []
        h = await sb.pty.create(
            e2b.PtySize(rows=size[0], cols=size[1]), on_data=o.append, timeout=30
        )
        await sb.pty.send_stdin(h.pid, b"echo SZ[$(stty size)]; exit\n")
        await probe(h.wait())
        out[label] = _field(await _text(o), "SZ")
    return out


@case
async def cmd_stream_input(sb):
    if not hasattr(sb.commands, "stream_input"):
        return {"since": "2.58"}
    path = "/tmp/conf-stream-input.txt"
    h = await sb.commands.run(f"cat > {path}", background=True, stdin=True, timeout=60)
    inp = sb.commands.stream_input(h.pid)
    res = {"handle_type": type(inp).__name__}
    await inp.send("a\n")
    await inp.send(b"b\n")
    await inp.send("c\n")
    res["close"] = await probe(inp.close())
    res["wait_after_close"] = await probe(h.wait(), result_info, timeout=6)
    if "hung" in res["wait_after_close"]:
        await sb.commands.close_stdin(h.pid)
        await probe(h.wait())
    res["file"] = await sb.files.read(path)
    res["send_after_close"] = await probe(inp.send("late"))
    res["close_twice"] = await probe(inp.close())
    unknown = sb.commands.stream_input(999999)
    res["unknown_send"] = await probe(unknown.send("x"))
    res["unknown_close"] = await probe(unknown.close())

    async def ctx_unknown():
        async with sb.commands.stream_input(999998) as ctx:
            return type(ctx).__name__

    res["context_manager_unknown"] = await probe(ctx_unknown())
    return res


@case
async def cmd_pty_stream_input(sb):
    if not hasattr(sb.pty, "stream_input"):
        return {"since": "2.58"}
    out = []
    h = await sb.pty.create(e2b.PtySize(rows=24, cols=80), on_data=out.append, timeout=60)
    inp = sb.pty.stream_input(h.pid)
    await inp.send("echo S1-$((1+1))\n")
    await inp.send(b"echo S2-$((2+2))\n")
    await inp.send("exit 4\n")
    await _wait_for(out, "S2-4", 10)
    text = await _text(out)
    res = {
        "ordered": "S1-2" in text and "S2-4" in text and text.index("S1-2") < text.index("S2-4"),
        "close": await probe(inp.close()),
        "exit": await probe(h.wait(), result_info, timeout=15),
        "send_after_close": await probe(inp.send("late")),
    }
    unknown = sb.pty.stream_input(999999)
    res["unknown_send"] = await probe(unknown.send("x"))
    res["unknown_close"] = await probe(unknown.close())
    return res


@case
async def cmd_run_as_root_and_env(sb):
    if ROOT == {}:
        return None
    r = await sb.commands.run("id -un; echo $HOME; pwd", user="root")
    return r.stdout.split("\n")
