"""sandbox.process: exec, code run, sessions, async commands and their logs,
input, PTY. `mock` cases use the mock guest's simulated commands (echo, fail
N, sleep MS); the rest need a real shell.

Derived from daytona 0.225.0 `_sync/process.py`, `common/process.py`,
`common/pty.py`, `handle/pty_handle.py`."""

import asyncio
import time

from harness import case, err_obs


@case("process.exec_result_and_exit_code", needs="mock", ref="_sync/process.py exec")
async def exec_basic(c):
    sb = await c.main()
    r = await c.w(sb.process.exec("echo hi there"))
    assert (r.exit_code, r.result) == (0, "hi there\n"), (r.exit_code, r.result)
    assert r.artifacts.stdout == r.result and r.artifacts.charts == [], r.artifacts
    r = await c.w(sb.process.exec("exit 3"))
    assert r.exit_code == 3, r.exit_code
    return {"echo": "hi there\n", "fail": 3}


@case(
    "process.exec_timeout_error",
    needs="mock",
    ref="common/errors.py PROCESS_EXECUTION_TIMEOUT (408)",
)
async def exec_timeout(c):
    sb = await c.main()
    t0 = time.time()
    e = await c.raises(lambda: sb.process.exec("sleep 20000", timeout=1))
    assert e and e["type"] == "DaytonaProcessExecutionTimeoutError", e
    assert (
        e["status_code"] == 408
        and e["code"] == "PROCESS_EXECUTION_TIMEOUT"
        and e["source"] == "DAYTONA_DAEMON"
    ), e
    assert time.time() - t0 < 15, "the command was not stopped at its timeout"
    return err_obs(e)


@case("process.exec_cwd_env_user", needs="space", ref="_sync/process.py exec (cwd, env)")
async def exec_cwd_env(c):
    sb = await c.main()
    r = await c.w(sb.process.exec("pwd", cwd="/tmp"))
    assert r.result.strip() == "/tmp", r.result
    r = await c.w(sb.process.exec('echo "$FOO:$BAR"', env={"FOO": "bar", "BAR": "two words"}))
    assert r.result.strip() == "bar:two words", r.result
    r = await c.w(sb.process.exec("whoami"))
    assert r.result.strip() == "daytona", r.result
    r = await c.w(sb.process.exec("mkdir -p /tmp/rel/x && pwd", cwd="/tmp/rel/x"))
    home = (await c.w(sb.process.exec("echo $HOME"))).result.strip()
    rel = await c.w(sb.process.exec("pwd", cwd="rel-dir-from-home"))
    return {
        "cwd": "/tmp",
        "env": "bar:two words",
        "user": "daytona",
        "relativeCwdFailsOrResolves": rel.exit_code in (0, 1, 2),
    }


@case(
    "process.exec_shell_semantics",
    needs="space",
    ref="_sync/process.py exec (first of zsh, bash, sh)",
)
async def exec_shell(c):
    sb = await c.main()
    r = await c.w(sb.process.exec("echo a; echo b >&2; exit 5"))
    assert r.exit_code == 5 and "a" in r.result and "b" in r.result, (r.exit_code, r.result)
    r = await c.w(sb.process.exec("cat <<'EOF'\nline1\nline2\nEOF"))
    assert r.result == "line1\nline2\n", repr(r.result)
    r = await c.w(sb.process.exec("true && echo yes || echo no"))
    assert r.result.strip() == "yes"
    r = await c.w(sb.process.exec("nohup sh -c 'sleep 60' > /tmp/bg.log 2>&1 &"))
    assert r.exit_code == 0
    return {"exit": 5, "heredoc": True, "background": True}


@case("process.exec_large_output", needs="space", ref="_sync/process.py exec")
async def exec_large(c):
    sb = await c.main()
    r = await c.w(sb.process.exec("head -c 3000000 /dev/zero | tr '\\0' 'x'"))
    assert r.exit_code == 0 and len(r.result) == 3_000_000, len(r.result)
    return {"bytes": len(r.result)}


@case("process.code_run_python", needs="space", ref="_sync/process.py code_run")
async def code_run(c):
    sb = await c.main()
    sdk = c.sdk
    r = await c.w(
        sb.process.code_run(
            "import sys; print(6*7, sys.argv[1:])", params=sdk.CodeRunParams(argv=["a"])
        )
    )
    assert r.exit_code == 0 and r.result.strip() == "42 ['a']", (r.exit_code, r.result)
    r = await c.w(
        sb.process.code_run(
            "import os; print(os.environ['K'])", params=sdk.CodeRunParams(env={"K": "v"})
        )
    )
    assert r.result.strip() == "v", r.result
    r = await c.w(sb.process.code_run("raise SystemExit(4)"))
    assert r.exit_code == 4, r.exit_code
    r = await c.w(sb.process.code_run("1/0"))
    assert r.exit_code != 0 and "ZeroDivisionError" in r.result, (r.exit_code, r.result)
    return {"argv": "42 ['a']", "env": "v", "exit": 4, "traceback": True}


@case("process.code_run_timeout", needs="space", ref="_sync/process.py code_run (timeout)")
async def code_run_timeout(c):
    sb = await c.main()
    e = await c.raises(lambda: sb.process.code_run("import time; time.sleep(30)", timeout=1))
    # the SDK's code_run lets the toolbox's 408 through as the generated
    # client's ApiException on a real run, and maps it on the mock lane
    assert e and (e["type"] == "DaytonaProcessExecutionTimeoutError" or e["status_code"] == 408), e
    return {"timedOut": True, "status_code": 408}


@case(
    "process.session_state_carries_over",
    needs="space",
    ref="_sync/process.py create_session/execute_session_command",
)
async def sessions(c):
    sb = await c.main()
    sdk = c.sdk
    await c.w(sb.process.create_session("s1"))
    r = await c.w(
        sb.process.execute_session_command(
            "s1", sdk.SessionExecuteRequest(command="cd /tmp && export X=7")
        )
    )
    assert r.exit_code == 0, r.exit_code
    r = await c.w(
        sb.process.execute_session_command(
            "s1", sdk.SessionExecuteRequest(command='echo "$PWD:$X"')
        )
    )
    assert r.stdout.strip() == "/tmp:7", r.stdout
    r = await c.w(
        sb.process.execute_session_command(
            "s1", sdk.SessionExecuteRequest(command="echo out; echo err >&2; exit 3")
        )
    )
    assert r.exit_code == 3 and r.stdout.strip() == "out" and r.stderr.strip() == "err", (
        r.exit_code,
        r.stdout,
        r.stderr,
    )
    s = await c.w(sb.process.get_session("s1"))
    assert len(s.commands) == 3 and [x.exit_code for x in s.commands] == [0, 0, 3], [
        (x.command, x.exit_code) for x in s.commands
    ]
    assert "s1" in [x.session_id for x in await c.w(sb.process.list_sessions())]
    await c.w(sb.process.delete_session("s1"))
    e = await c.raises(lambda: sb.process.get_session("s1"))
    assert e and e["status_code"] == 404, e
    return {"state": "/tmp:7", "exitCodes": [0, 0, 3], "deleted": err_obs(e)}


@case(
    "process.session_async_command_and_logs",
    needs="space",
    ref="_sync/process.py execute_session_command(run_async)/get_session_command/get_session_command_logs",
)
async def session_async(c):
    sb = await c.main()
    sdk = c.sdk
    await c.w(sb.process.create_session("sa"))
    a = await c.w(
        sb.process.execute_session_command(
            "sa",
            sdk.SessionExecuteRequest(command="sleep 1; echo late; echo bad >&2", run_async=True),
        )
    )
    assert a.cmd_id and a.exit_code is None, a
    c0 = await c.w(sb.process.get_session_command("sa", a.cmd_id))
    assert c0.exit_code is None, "still running"
    for _ in range(60):
        cmd = await c.w(sb.process.get_session_command("sa", a.cmd_id))
        if cmd.exit_code is not None:
            break
        (await asyncio.sleep(0.2)) if c.is_async else time.sleep(0.2)
    assert cmd.exit_code == 0, cmd
    logs = await c.w(sb.process.get_session_command_logs("sa", a.cmd_id))
    assert logs.stdout.strip() == "late" and logs.stderr.strip() == "bad", logs
    assert logs.output.count("late") == 1 and "bad" in logs.output, logs.output
    await c.w(sb.process.delete_session("sa"))
    return {"asyncPending": True, "stdout": "late", "stderr": "bad", "exit": 0}


@case(
    "process.session_follow_logs",
    needs="space",
    ref="_sync/process.py get_session_command_logs (on_stdout, on_stderr)",
)
async def session_follow(c):
    sb = await c.main()
    sdk = c.sdk
    await c.w(sb.process.create_session("sf"))
    a = await c.w(
        sb.process.execute_session_command(
            "sf",
            sdk.SessionExecuteRequest(
                command="for i in 1 2 3; do echo o$i; sleep 0.3; done; echo e >&2", run_async=True
            ),
        )
    )
    out, err = [], []
    # streamed logs are a coroutine in the sync client too
    if c.is_async:
        await sb.process.get_session_command_logs_async("sf", a.cmd_id, out.append, err.append)
    else:
        # a sync-mode case runs inside the harness's loop: stream on a thread's own
        import concurrent.futures

        with concurrent.futures.ThreadPoolExecutor(1) as ex:
            ex.submit(
                asyncio.run,
                sb.process.get_session_command_logs_async("sf", a.cmd_id, out.append, err.append),
            ).result()
    so, se = "".join(out), "".join(err)
    assert so.replace("\r", "").split() == ["o1", "o2", "o3"] and se.strip() == "e", (so, se)
    await c.w(sb.process.delete_session("sf"))
    return {"stdout": ["o1", "o2", "o3"], "stderr": "e"}


@case(
    "process.session_input_to_running_command",
    needs="space",
    ref="_sync/process.py send_session_command_input",
)
async def session_input(c):
    sb = await c.main()
    sdk = c.sdk
    await c.w(sb.process.create_session("si"))
    a = await c.w(
        sb.process.execute_session_command(
            "si", sdk.SessionExecuteRequest(command="read line; echo got:$line", run_async=True)
        )
    )
    await c.w(sb.process.send_session_command_input("si", a.cmd_id, "hello\n"))
    for _ in range(60):
        cmd = await c.w(sb.process.get_session_command("si", a.cmd_id))
        if cmd.exit_code is not None:
            break
        (await asyncio.sleep(0.2)) if c.is_async else time.sleep(0.2)
    logs = await c.w(sb.process.get_session_command_logs("si", a.cmd_id))
    assert "got:hello" in logs.stdout, logs
    e = await c.raises(lambda: sb.process.send_session_command_input("si", a.cmd_id, "late\n"))
    assert e and e["status_code"] == 410 and e["code"] == "COMMAND_ALREADY_COMPLETED", e
    await c.w(sb.process.delete_session("si"))
    return {"got": "hello", "afterCompletion": err_obs(e)}


@case(
    "process.session_command_ordering",
    needs="space",
    ref="_sync/process.py execute_session_command (commands run in order)",
)
async def session_order(c):
    sb = await c.main()
    sdk = c.sdk
    await c.w(sb.process.create_session("so"))
    ids = []
    for i in range(5):
        a = await c.w(
            sb.process.execute_session_command(
                "so",
                sdk.SessionExecuteRequest(
                    command=f"echo {i} >> /tmp/order.txt; sleep 0.2", run_async=True
                ),
            )
        )
        ids.append(a.cmd_id)
    for cid in ids:
        for _ in range(100):
            cmd = await c.w(sb.process.get_session_command("so", cid))
            if cmd.exit_code is not None:
                break
            (await asyncio.sleep(0.1)) if c.is_async else time.sleep(0.1)
    r = await c.w(sb.process.exec("cat /tmp/order.txt && rm /tmp/order.txt"))
    assert r.result.split() == ["0", "1", "2", "3", "4"], r.result
    await c.w(sb.process.delete_session("so"))
    return {"order": ["0", "1", "2", "3", "4"]}


@case("process.session_errors", needs="mock", ref="common/errors.py (404)")
async def session_errors(c):
    sb = await c.main()
    out = {}
    for name, fn in {
        "get": lambda: sb.process.get_session("no-such-session"),
        "delete": lambda: sb.process.delete_session("no-such-session"),
        "logs": lambda: sb.process.get_session_command_logs("no-such-session", "x"),
    }.items():
        e = await c.raises(fn)
        assert e and e["status_code"] == 404, (name, e)
        out[name] = err_obs(e)
    await c.w(sb.process.create_session("dupe"))
    dup = await c.raises(lambda: sb.process.create_session("dupe"))
    assert dup and dup["status_code"] in (400, 409), dup
    await c.w(sb.process.delete_session("dupe"))
    return {"missing": out, "duplicate": err_obs(dup)}


@case(
    "process.pty_roundtrip",
    needs="space",
    ref="_sync/process.py create_pty_session; handle/pty_handle.py",
)
async def pty(c):
    sb = await c.main()
    sdk = c.sdk
    out = []
    if c.is_async:
        h = await sb.process.create_pty_session(
            id="p1", on_data=out.append, pty_size=sdk.PtySize(cols=80, rows=24)
        )
        await h.wait_for_connection()
        await h.send_input("echo pty-$((6*7))\nexit\n")
        r = await h.wait()
    else:
        h = sb.process.create_pty_session(id="p1", pty_size=sdk.PtySize(cols=80, rows=24))
        h.send_input("echo pty-$((6*7))\nexit\n")
        r = h.wait(on_data=out.append, timeout=30)
    assert b"pty-42" in b"".join(out), out
    assert r.exit_code == 0, r
    return {"echo": "pty-42", "exit": 0}


@case(
    "process.pty_resize_list_kill",
    needs="space",
    ref="_sync/process.py resize_pty_session/list_pty_sessions/kill_pty_session",
)
async def pty_manage(c):
    sb = await c.main()
    sdk = c.sdk
    if c.is_async:
        await sb.process.create_pty_session(
            id="p2", on_data=lambda d: None, pty_size=sdk.PtySize(cols=80, rows=24)
        )
    else:
        sb.process.create_pty_session(id="p2", pty_size=sdk.PtySize(cols=80, rows=24))
    ids = [p.id for p in await c.w(sb.process.list_pty_sessions())]
    assert "p2" in ids, ids
    info = await c.w(sb.process.resize_pty_session("p2", sdk.PtySize(cols=120, rows=40)))
    assert (info.cols, info.rows) == (120, 40), info
    got = await c.w(sb.process.get_pty_session_info("p2"))
    assert got.id == "p2" and got.active, got
    await c.w(sb.process.kill_pty_session("p2"))
    e = await c.raises(lambda: sb.process.get_pty_session_info("p2"))
    assert e and e["status_code"] == 404, e
    return {"resized": [120, 40], "killed": err_obs(e)}


@case(
    "process.background_server_and_port",
    needs="space",
    ref="_sync/process.py exec (background process); sandbox.py get_preview_link",
)
async def background_server(c):
    sb = await c.main()
    await c.w(sb.process.exec("mkdir -p /tmp/www9 && echo -n up > /tmp/www9/index.html"))
    await c.w(
        sb.process.exec("cd /tmp/www9 && nohup python3 -m http.server 9009 > /tmp/www9.log 2>&1 &")
    )
    for _ in range(40):
        r = await c.w(sb.process.exec("curl -s localhost:9009/index.html || true"))
        if r.result.strip() == "up":
            break
        (await asyncio.sleep(0.5)) if c.is_async else time.sleep(0.5)
    assert r.result.strip() == "up", r.result
    return {"served": "up"}
