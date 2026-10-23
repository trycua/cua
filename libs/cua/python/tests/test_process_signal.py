"""`ExitInfo.signal_number` / `signal_name` report any terminating signal.

`ExitInfo.signal` only names the signals of the contract enum (kill, term,
...). A process that dies of SIGSEGV, SIGABRT or SIGPIPE leaves it unset;
the raw number and the conventional name say what happened. Against the
fixture's in-process cua-spacesd core (real child processes).
"""

from __future__ import annotations

import asyncio
import signal as _signal

import cua


async def run_sh(env, script):
    proc = await env.spawn(cua.SpacesdCommand(program="sh", args=["-c", script]))
    while (ev := await proc.next_event()) is not None:
        if ev.kind == cua.ProcessEventKind.EXIT:
            return ev.exit
    raise AssertionError("no exit event")


def test_signal_fields_default_to_none():
    info = cua.ExitInfo(code=0, signal=None, timed_out=False, error=None, success=True)
    assert info.signal_number is None and info.signal_name is None


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
