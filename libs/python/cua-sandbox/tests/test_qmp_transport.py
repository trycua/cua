"""QMPTransport handles the ``path`` form that ``Mouse`` sends; no QEMU is started."""

from __future__ import annotations

from cua_sandbox.interfaces.mouse import Mouse
from cua_sandbox.transport.qmp import QMPTransport


class _RecordingQMP(QMPTransport):
    def __init__(self):
        super().__init__()
        self.calls: list[tuple] = []

    async def _mouse_down(self, x, y, button="left", **_):
        self.calls.append(("down", x, y, button))

    async def _mouse_move(self, x, y, **_):
        self.calls.append(("move", x, y))

    async def _mouse_up(self, x, y, button="left", **_):
        self.calls.append(("up", x, y, button))


async def test_mouse_drag_presses_moves_and_releases():
    qmp = _RecordingQMP()
    await Mouse(qmp).drag(10, 10, 200, 100, button="right")
    assert qmp.calls[0] == ("down", 10, 10, "right")
    assert qmp.calls[-1] == ("up", 200, 100, "right")
    moves = qmp.calls[1:-1]
    assert len(moves) == 10 and moves[-1] == ("move", 200, 100)


async def test_drag_walks_every_point_of_a_longer_path():
    qmp = _RecordingQMP()
    await qmp.send("drag", path=[[0, 0], [100, 0], [100, 50]])
    moves = [c[1:] for c in qmp.calls if c[0] == "move"]
    assert (100, 0) in moves and moves[-1] == (100, 50)
    assert qmp.calls[-1] == ("up", 100, 50, "left")
