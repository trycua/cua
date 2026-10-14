"""OSWorld-G grounding on cua-bench: 564 screenshots, no environment.

xlang-ai/OSWorld-G (Apache-2.0) at a pinned commit: ``benchmark/OSWorld-G.json``
(sha256-pinned) plus each item's PNG, fetched lazily and cached. Tasks
declare ``provider: "dataset"``: ``cb run`` starts no sandbox, the agent sees
the screenshot and clicks; the first click must land in the target box
(``bbox`` x, y, w, h) or polygon. Refusal items (54) score only when the
agent reports the task infeasible (``session.report_infeasible()``) without
clicking.

::

    cb run dataset libs/cua-bench/tasks/osworld_g --agent <agent>
    cb run dataset libs/cua-bench/tasks/osworld_g --oracle --max-variants 20
"""

from __future__ import annotations

import json
from typing import Any, Optional

import cua_bench as cb
from cua_bench.adapters.datasets import cache_dir, fetch_url
from cua_bench.adapters.grounding import GroundingAdapter, clicks, in_polygon, in_xyxy, polygon_centroid

COMMIT = "daa6bd8e0e629f0917ad2984df930bf0bd967540"
RAW = f"https://raw.githubusercontent.com/xlang-ai/OSWorld-G/{COMMIT}/benchmark"
JSON_SHA256 = "8d8f210461a16702b99410658b605708f963b70aa5fc03fca897b4bafd9e3962"


def load_items(path: Optional[str] = None) -> list[dict]:
    path = path or str(fetch_url(f"{RAW}/OSWorld-G.json", cache_dir("osworld-g") / COMMIT / "OSWorld-G.json",
                                 JSON_SHA256))
    return json.loads(open(path, encoding="utf-8").read())


class OSWorldGAdapter(GroundingAdapter):
    id = "osworld-g"
    version = f"OSWorld-G@{COMMIT[:7]}"

    def __init__(self, data: Optional[str] = None, images_dir: Optional[str] = None) -> None:
        self._data, self._images = data, images_dir

    def load_tasks(self, split: str) -> list[cb.Task]:
        return [
            cb.Task(description=item["instruction"], task_id=f"osworld-g-{item['id']}",
                    metadata={"item": item, "image_size": item.get("image_size"),
                              "box_type": item["box_type"], "gui_types": item.get("GUI_types", [])})
            for item in load_items(self._data)
        ]

    def image_bytes(self, task: cb.Task) -> bytes:
        name = task.metadata["item"]["image_path"]
        if self._images:
            return open(f"{self._images}/{name}", "rb").read()
        return fetch_url(f"{RAW}/images/{name}", cache_dir("osworld-g") / COMMIT / "images" / name).read_bytes()

    def point_ok(self, task: cb.Task, point: tuple) -> bool:
        item = task.metadata["item"]
        c = item["box_coordinates"]
        if item["box_type"] == "bbox":
            x, y, w, h = c
            return in_xyxy(point, (x, y, x + w, y + h))
        if item["box_type"] == "polygon":
            return in_polygon(point, c)
        return False

    def target(self, task: cb.Task) -> Optional[tuple[float, float]]:
        item = task.metadata["item"]
        c = item["box_coordinates"]
        if item["box_type"] == "bbox":
            return (c[0] + c[2] / 2, c[1] + c[3] / 2)
        if item["box_type"] == "polygon":
            return polygon_centroid(c)
        return None

    async def evaluate(self, task: cb.Task, session: Any, ep: Any) -> float:
        points = clicks(session)
        if task.metadata["box_type"] == "refusal":
            return 1.0 if session.infeasible and not points else 0.0
        return 1.0 if points and self.point_ok(task, points[0]) else 0.0


ADAPTER = OSWorldGAdapter().register(globals())
