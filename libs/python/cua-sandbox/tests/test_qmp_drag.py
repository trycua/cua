"""Mouse.drag over the QMP transport presses at the start and releases at the end."""

import pytest
from cua_sandbox.interfaces.mouse import Mouse
from cua_sandbox.transport.qmp import QMPTransport


class _RecordingQMP(QMPTransport):
    def __init__(self):
        super().__init__()
        self.sent = []

    async def _execute(self, command, arguments=None):
        self.sent.append((command, arguments))
        return {"return": {}}


def _button_events(sent):
    return [
        e["data"]["down"]
        for _, args in sent
        for e in (args or {}).get("events", [])
        if e["type"] == "btn"
    ]


async def test_mouse_drag_presses_moves_and_releases():
    t = _RecordingQMP()
    await Mouse(t).drag(10, 10, 200, 200)
    assert _button_events(t.sent) == [True, False]
    # Press, ten moves, release.
    assert len(t.sent) == 12


async def test_drag_follows_every_point_of_a_path():
    t = _RecordingQMP()
    await t.send("drag", path=[[0, 0], [100, 0], [100, 100]], button="left")
    assert _button_events(t.sent) == [True, False]
    # Press, ten moves per segment, release.
    assert len(t.sent) == 22


async def test_drag_still_accepts_the_start_and_end_form():
    t = _RecordingQMP()
    await t.send("drag", start_x=1, start_y=2, end_x=30, end_y=40)
    assert _button_events(t.sent) == [True, False]


async def test_drag_without_a_destination_is_refused_plainly():
    t = _RecordingQMP()
    with pytest.raises(TypeError, match="path"):
        await t.send("drag", button="left")
    with pytest.raises(ValueError):
        await t.send("drag", path=[[1, 1]])
    assert t.sent == []
