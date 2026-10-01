"""ScreenSpot-Pro grounding on cua-bench: ~1,581 professional-app screenshots, no environment.

likaixin/ScreenSpot-Pro on Hugging Face (MIT) at a pinned revision:
``annotations/<app>_<os>.json`` and ``images/...``, fetched per item and
cached (the full set is about 3.3 GB). Tasks declare ``provider:
"dataset"``: no sandbox; the agent sees the screenshot and must answer with
exactly one click, inside the target bbox (x1, y1, x2, y2).

::

    cb run dataset libs/cua-bench/tasks/screenspot_pro --agent <agent>
    CUA_BENCH_SSPRO_APPS=excel_macos,vscode_macos cb run dataset libs/cua-bench/tasks/screenspot_pro --oracle
"""

from __future__ import annotations

import json
import os
from typing import Any, Optional

import cua_bench as cb
from cua_bench.adapters.datasets import hf_file
from cua_bench.adapters.grounding import GroundingAdapter, clicks, in_xyxy

HF_REPO = "likaixin/ScreenSpot-Pro"
HF_REVISION = "210e78d3844251110bff86c95835ebd37a6930fa"
ANNOTATIONS = (
    "android_studio_macos autocad_windows blender_windows davinci_macos eviews_windows excel_macos "
    "fruitloops_windows illustrator_windows inventor_windows linux_common_linux macos_common_macos "
    "matlab_macos origin_windows photoshop_windows powerpoint_windows premiere_windows pycharm_macos "
    "quartus_windows solidworks_windows stata_windows unreal_engine_windows vivado_windows "
    "vmware_macos vscode_macos windows_common_windows word_macos"
).split()


def load_items(apps: Optional[list[str]] = None, annotations_dir: Optional[str] = None) -> list[dict]:
    items = []
    for name in apps or ANNOTATIONS:
        if name not in ANNOTATIONS:
            raise ValueError(f"unknown ScreenSpot-Pro annotation set {name!r}")
        path = (f"{annotations_dir}/{name}.json" if annotations_dir
                else hf_file(HF_REPO, f"annotations/{name}.json", HF_REVISION, name="screenspot-pro"))
        items += json.loads(open(path, encoding="utf-8").read())
    return items


class ScreenSpotProAdapter(GroundingAdapter):
    id = "screenspot-pro"
    version = f"ScreenSpot-Pro@{HF_REVISION[:7]}"

    def __init__(self, annotations_dir: Optional[str] = None, images_dir: Optional[str] = None) -> None:
        self._ann, self._images = annotations_dir, images_dir

    def load_tasks(self, split: str) -> list[cb.Task]:
        apps = [a.strip() for a in os.environ.get("CUA_BENCH_SSPRO_APPS", "").split(",") if a.strip()]
        return [
            cb.Task(description=item["instruction"], task_id=f"sspro-{item['id']}",
                    metadata={"item": item, "image_size": item.get("img_size"),
                              "application": item.get("application"), "platform": item.get("platform"),
                              "ui_type": item.get("ui_type"), "group": item.get("group")})
            for item in load_items(apps or None, self._ann)
        ]

    def image_bytes(self, task: cb.Task) -> bytes:
        name = task.metadata["item"]["img_filename"]
        if self._images:
            return open(f"{self._images}/{name}", "rb").read()
        return hf_file(HF_REPO, f"images/{name}", HF_REVISION, name="screenspot-pro").read_bytes()

    def target(self, task: cb.Task) -> Optional[tuple[float, float]]:
        x0, y0, x1, y1 = task.metadata["item"]["bbox"]
        return ((x0 + x1) / 2, (y0 + y1) / 2)

    async def evaluate(self, task: cb.Task, session: Any, ep: Any) -> float:
        points = clicks(session)
        # Exactly one point action (the upstream protocol).
        return 1.0 if len(points) == 1 and in_xyxy(points[0], task.metadata["item"]["bbox"]) else 0.0


ADAPTER = ScreenSpotProAdapter().register(globals())
