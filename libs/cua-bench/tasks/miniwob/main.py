"""MiniWoB++ on cua-bench (bench-web image), built on ``BenchAdapter``.

130 synthetic web tasks (Farama-Foundation/miniwob-plusplus, MIT, at the
commit the image bakes in), served inside the guest on :7560 and opened in
the guest's Chromium. Each variant is a (task, seed) pair: setup loads the
page through bench-web-ctl, seeds the page's RNG, lengthens the episode
timer and starts the episode, then makes the page's own instruction the
task description. The page's JavaScript computes the reward; evaluate reads
it (score 1.0 when the raw reward is positive, as MiniWoB counts success).
No network is needed in the guest.

::

    cb run dataset libs/cua-bench/tasks/miniwob --agent <agent>
    cb run dataset libs/cua-bench/tasks/miniwob --oracle        # the tasks with scripted solutions
    cb run dataset libs/cua-bench/tasks/miniwob --kind vm --on cloud

``CUA_BENCH_MINIWOB_TASKS`` (comma list) and ``CUA_BENCH_MINIWOB_SEEDS``
(seeds per task, default 1) select the variants; ``CUA_BENCH_MINIWOB_SECONDS``
sets the episode time limit (default 600).
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

import cua_bench as cb
from cua_bench.adapters.web import BenchWebAdapter, evaluate_js, reset

MINIWOB_COMMIT = "a7658beb68e1d3572329e06734c5822bd284ddc8"
PAGE = "http://127.0.0.1:7560/miniwob/{task}.html"
TASKS = [t for t in (Path(__file__).parent / "tasks.txt").read_text().split() if t]

#: Scripted solutions (JavaScript run in the task page after the episode starts).
ORACLES = {
    "click-button": """(() => { const t = document.querySelector('#query').textContent.match(/"(.*)"/)[1];
        [...document.querySelectorAll('#area button')].find(b => b.textContent === t).click(); })()""",
    "click-link": """(() => { const t = document.querySelector('#query').textContent.match(/"(.*)"/)[1];
        [...document.getElementsByClassName('alink')].find(a => a.textContent === t).click(); })()""",
    "enter-text": """(() => { document.getElementById('tt').value = document.querySelector('#query .bold').textContent;
        document.getElementById('subbtn').click(); })()""",
    "focus-text": "document.getElementById('tt').focus()",
    "click-dialog": "document.querySelector('button.ui-button').click()",
}

START = """(() => {{
  Math.seedrandom({seed});
  core.EPISODE_MAX_TIME = {ms};
  core.startEpisodeReal();
  return document.querySelector('#query').innerText;
}})()"""

RESULT = "({done: WOB_DONE_GLOBAL, raw: WOB_RAW_REWARD_GLOBAL, reward: WOB_REWARD_GLOBAL, reason: WOB_REWARD_REASON})"


def score(result: dict) -> float:
    """MiniWoB success: the page ended the episode with a positive raw reward."""
    return 1.0 if result.get("done") and float(result.get("raw") or 0) > 0 else 0.0


class MiniWoBAdapter(BenchWebAdapter):
    id = "miniwob"
    version = f"miniwob-plusplus@{MINIWOB_COMMIT[:7]}"

    def load_tasks(self, split: str) -> list[cb.Task]:
        names = [t.strip() for t in os.environ.get("CUA_BENCH_MINIWOB_TASKS", "").split(",") if t.strip()]
        unknown = [n for n in names if n not in TASKS]
        if unknown:
            raise ValueError(f"unknown MiniWoB tasks: {unknown}")
        seeds = int(os.environ.get("CUA_BENCH_MINIWOB_SEEDS", "1"))
        return [
            cb.Task(
                description=f"MiniWoB++ {name}: follow the instruction at the top of the page.",
                task_id=f"miniwob-{name}-s{seed}",
                metadata={"miniwob": name, "seed": seed, "has_oracle": name in ORACLES},
            )
            for name in (names or TASKS)
            for seed in range(seeds)
        ]

    async def setup(self, task: cb.Task, session: Any, ep: Any) -> None:
        await reset(ep, PAGE.format(task=task.metadata["miniwob"]))
        ms = int(float(os.environ.get("CUA_BENCH_MINIWOB_SECONDS", "600")) * 1000)
        query = await evaluate_js(ep, START.format(seed=json.dumps(str(task.metadata["seed"])), ms=ms))
        # The page generates the instruction; the agent gets exactly that.
        task.description = f"{query} (MiniWoB++ {task.metadata['miniwob']}, in the browser window)"

    async def evaluate(self, task: cb.Task, session: Any, ep: Any) -> float:
        result = await evaluate_js(ep, RESULT) or {}
        print(f"MiniWoB {task.metadata['miniwob']}: {json.dumps(result)}")
        return score(result)

    async def oracle(self, task: cb.Task, session: Any, ep: Any) -> None:
        js = ORACLES.get(task.metadata["miniwob"])
        if js is None:
            raise LookupError(f"no scripted solution for MiniWoB {task.metadata['miniwob']} "
                              f"(have: {', '.join(sorted(ORACLES))})")
        await evaluate_js(ep, js)


ADAPTER = MiniWoBAdapter().register(globals())
