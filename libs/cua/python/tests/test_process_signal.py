"""`ExitInfo.signal_number` / `signal_name` report any terminating signal.

`ExitInfo.signal` only names the signals of the contract enum (kill, term,
...). A process that dies of SIGSEGV, SIGABRT or SIGPIPE leaves it unset;
the raw number and the conventional name say what happened. Against the
fixture's in-process cua-spacesd core (real child processes).
"""

from __future__ import annotations

import asyncio
import os
import signal as _signal
import sys

import cua
import pytest


async def run_program(env, program, *args):
    proc = await env.spawn(cua.SpacesdCommand(program=program, args=list(args)))
    while (ev := await proc.next_event()) is not None:
        if ev.kind == cua.ProcessEventKind.EXIT:
            return ev.exit
    raise AssertionError("no exit event")


async def run_sh(env, script):
    return await run_program(env, "sh", "-c", script)


def test_signal_fields_default_to_none():
    info = cua.ExitInfo(code=0, signal=None, timed_out=False, error=None, success=True)
    assert info.signal_number is None and info.signal_name is None


@pytest.mark.skipif(
    os.name == "nt", reason="POSIX signals: a Windows process ends with an exit code"
)
def test_signal_deaths_carry_number_and_name(fixtures, tmp_path):
    async def body():
        c = cua.embedded(
            state_dir=str(tmp_path / "sbx"),
            spaces_home=str(tmp_path / "cua"),
        )
        env = await c.spacesd(fixtures["spaces_url"], fixtures["spaces_token"])

        # Outside the contract enum: only the raw fields say which signal.
        for name in ("SEGV", "ABRT", "PIPE"):
            exit_info = await run_sh(env, f"kill -{name} $$; sleep 5")
            assert exit_info.code is None and not exit_info.success
            assert exit_info.signal is None, exit_info
            assert exit_info.signal_name == name
            assert exit_info.signal_number == int(getattr(_signal, "SIG" + name))

        # Inside the enum: both are set.
        exit_info = await run_sh(env, "kill -KILL $$; sleep 5")
        assert exit_info.signal == "kill"
        assert (exit_info.signal_number, exit_info.signal_name) == (9, "KILL")

        # A normal exit has no signal at all.
        exit_info = await run_sh(env, "exit 3")
        assert exit_info.code == 3
        assert exit_info.signal_number is None and exit_info.signal_name is None

    asyncio.run(asyncio.wait_for(body(), timeout=60))


# A Windows process never dies of a POSIX signal: even when it ends abnormally
# the native exit code says how, and every signal field stays unset. The second
# case ends through TerminateProcess, the call behind `Process.kill()` there.
_WINDOWS_TERMINATE_SELF = (
    "import ctypes, ctypes.wintypes as w;"
    "k = ctypes.windll.kernel32;"
    "k.GetCurrentProcess.restype = w.HANDLE;"
    "k.TerminateProcess.argtypes = [w.HANDLE, w.UINT];"
    "k.TerminateProcess(k.GetCurrentProcess(), 7)"
)


@pytest.mark.skipif(os.name != "nt", reason="Windows exit codes; POSIX is covered above")
@pytest.mark.parametrize(
    ("snippet", "native_code"),
    [
        ("import os; os._exit(3)", 3),
        (_WINDOWS_TERMINATE_SELF, 7),
    ],
    ids=["os-exit", "terminate-process"],
)
def test_windows_abnormal_termination_reports_native_code_and_no_signal(
    fixtures, tmp_path, snippet, native_code
):
    async def body():
        c = cua.embedded(
            state_dir=str(tmp_path / "sbx"),
            spaces_home=str(tmp_path / "cua"),
        )
        env = await c.spacesd(fixtures["spaces_url"], fixtures["spaces_token"])

        exit_info = await run_program(env, sys.executable, "-c", snippet)
        assert exit_info.code == native_code, exit_info
        assert not exit_info.success, exit_info
        assert not exit_info.timed_out, exit_info
        assert exit_info.signal is None, exit_info
        assert exit_info.signal_number is None and exit_info.signal_name is None

    asyncio.run(asyncio.wait_for(body(), timeout=60))


def test_a_normal_exit_carries_its_code_and_no_signal_on_every_os(fixtures, tmp_path):
    async def body():
        c = cua.embedded(
            state_dir=str(tmp_path / "sbx"),
            spaces_home=str(tmp_path / "cua"),
        )
        env = await c.spacesd(fixtures["spaces_url"], fixtures["spaces_token"])

        exit_info = await run_program(env, sys.executable, "-c", "raise SystemExit(3)")
        assert exit_info.code == 3 and not exit_info.success
        assert exit_info.signal is None
        assert exit_info.signal_number is None and exit_info.signal_name is None

        exit_info = await run_program(env, sys.executable, "-c", "")
        assert exit_info.code == 0 and exit_info.success
        assert exit_info.signal_number is None and exit_info.signal_name is None

    asyncio.run(asyncio.wait_for(body(), timeout=60))
