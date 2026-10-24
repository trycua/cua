"""`SpacesdCommand.scrollback_bytes` sizes the guest's output scrollback ring.

The guest keeps a process's output in a bounded ring and never blocks the
process, so a reader that falls behind by more than the ring loses the
evicted bytes (an offset gap in `ProcessEvent.offset`). Against the
fixture's in-process cua-spacesd core (real child processes), a consumer
that starts reading only after the process finished still gets all 16 MiB,
in order, when the ring is 64 MiB; with a 1 MiB ring the same reader sees
the gap.
"""

from __future__ import annotations

import asyncio

import cua

MIB = 1024 * 1024
SIZE = 16 * MIB
LINE = b"0123456789abcdefghijklmnopqrstuvwxyz\n"


def producer(scrollback_bytes=None):
    return cua.SpacesdCommand(
        program="sh",
        args=["-c", f"yes 0123456789abcdefghijklmnopqrstuvwxyz | head -c {SIZE}"],
        scrollback_bytes=scrollback_bytes,
    )


async def slow_read(proc):
    """Reads after the process finished and then slowly; returns
    (bytes received, first offset gap or None, exit)."""
    await asyncio.sleep(2)
    got = bytearray()
    gap = None
    while (ev := await proc.next_event()) is not None:
        if ev.kind == cua.ProcessEventKind.EXIT:
            return bytes(got), gap, ev.exit
        if ev.offset != len(got) and gap is None:
            gap = (len(got), ev.offset)
        got += ev.data
        if len(got) % (4 * MIB) < len(ev.data):
            await asyncio.sleep(0.2)
    raise AssertionError("no exit event")


def test_field_defaults_to_none():
    assert cua.SpacesdCommand(program="x").scrollback_bytes is None
    assert cua.SpacesdCommand(program="x", scrollback_bytes=5).scrollback_bytes == 5


def test_large_output_read_slowly_arrives_whole(fixtures, tmp_path):
    async def body():
        c = cua.embedded(
            state_dir=str(tmp_path / "sbx"),
            spaces_home=str(tmp_path / "cua"),
        )
        env = await c.spacesd(fixtures["spaces_url"], fixtures["spaces_token"])

        proc = await env.spawn(producer(64 * MIB))
        data, gap, exit_info = await slow_read(proc)
        assert exit_info.success
        assert gap is None, f"offset gap {gap}"
        assert len(data) == SIZE
        assert data == (LINE * (SIZE // len(LINE) + 1))[:SIZE]

        # Control: a 1 MiB ring loses what the slow reader did not take in time.
        proc = await env.spawn(producer(MIB))
        data, gap, exit_info = await slow_read(proc)
        assert exit_info.success
        assert gap is not None and len(data) < SIZE

        # Over the server maximum is clamped, not refused.
        proc = await env.spawn(producer(2**40))
        data, gap, _ = await slow_read(proc)
        assert gap is None and len(data) == SIZE

    asyncio.run(asyncio.wait_for(body(), timeout=120))
