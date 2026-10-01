"""GUI grounding sets as dataset tasks (no environment).

A grounding item is a screenshot, an instruction and a target region. The
task's session is a :class:`~cua_bench.computers.dataset.DatasetSession`:
setup shows the screenshot, the agent answers with a click (any agent that
drives ``session.execute_action``), and evaluate checks the recorded point.

* Point actions are clicks (left, right, double, middle); pointer moves do
  not count.
* A refusal item (the target is not on the screen) scores only when the
  agent reports it: ``session.report_infeasible()`` and no click.
"""

from __future__ import annotations

from typing import Any, Iterable, Optional, Sequence

from .base import BenchAdapter

POINT_TYPES = ("click", "double_click", "right_click", "middle_click")


def clicks(session: Any) -> list[tuple[float, float]]:
    return [(a["x"], a["y"]) for a in getattr(session, "actions", []) if a.get("type") in POINT_TYPES]


def in_xyxy(point: tuple, box: Sequence[float]) -> bool:
    x, y = point
    x0, y0, x1, y1 = box
    return min(x0, x1) <= x <= max(x0, x1) and min(y0, y1) <= y <= max(y0, y1)


def in_polygon(point: tuple, flat: Sequence[float]) -> bool:
    """Even-odd rule; points on an edge count as inside."""
    x, y = point
    pts = list(zip(flat[0::2], flat[1::2]))
    inside = False
    for (x0, y0), (x1, y1) in zip(pts, pts[1:] + pts[:1]):
        # on the edge
        if min(x0, x1) <= x <= max(x0, x1) and min(y0, y1) <= y <= max(y0, y1):
            if abs((x1 - x0) * (y - y0) - (y1 - y0) * (x - x0)) < 1e-9:
                return True
        if (y0 > y) != (y1 > y):
            if x < (x1 - x0) * (y - y0) / (y1 - y0) + x0:
                inside = not inside
    return inside


def polygon_centroid(flat: Sequence[float]) -> tuple[float, float]:
    pts = list(zip(flat[0::2], flat[1::2]))
    a = cx = cy = 0.0
    for (x0, y0), (x1, y1) in zip(pts, pts[1:] + pts[:1]):
        cross = x0 * y1 - x1 * y0
        a += cross
        cx += (x0 + x1) * cross
        cy += (y0 + y1) * cross
    if abs(a) < 1e-9:
        return (sum(p[0] for p in pts) / len(pts), sum(p[1] for p in pts) / len(pts))
    return (cx / (3 * a), cy / (3 * a))


class GroundingAdapter(BenchAdapter):
    """Base for grounding sets: no sandbox, one screenshot per task."""

    provider = "dataset"
    kinds = ()

    def image_bytes(self, task: Any) -> bytes:
        raise NotImplementedError

    async def setup(self, task: Any, session: Any, ep: Any) -> None:
        import asyncio

        png = await asyncio.to_thread(self.image_bytes, task)
        w, h = task.metadata.get("image_size") or (None, None)
        session.show(png, w, h)

    def point_ok(self, task: Any, point: tuple) -> bool:
        raise NotImplementedError

    def target(self, task: Any) -> Optional[tuple[float, float]]:
        """A point inside the target (the oracle's click), None for refusals."""
        raise NotImplementedError

    async def oracle(self, task: Any, session: Any, ep: Any) -> None:
        point = self.target(task)
        if point is None:
            await session.report_infeasible("the target is not on this screen")
        else:
            await session.click(round(point[0]), round(point[1]))


def first(items: Iterable) -> Any:
    return next(iter(items), None)
