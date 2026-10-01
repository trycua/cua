"""WebVoyager on cua-bench (bench-web image, live web), built on ``BenchAdapter``.

643 tasks on 15 live websites (MinorJerry/WebVoyager, Apache-2.0, data at a
pinned commit). Setup opens the task's start page in the guest's Chromium;
the agent browses with cua-driver and leaves its answer in
``session.final_answer``; evaluate asks the upstream auto-eval judge (its
prompt, its SUCCESS / NOT SUCCESS rule) about the final screenshots and the
answer. Needs egress in the guest and ``OPENAI_API_KEY``.

::

    cb run dataset libs/cua-bench/tasks/webvoyager --agent <agent> --max-variants 10
    CUA_BENCH_WEBVOYAGER_SITES=Allrecipes,ArXiv cb run dataset libs/cua-bench/tasks/webvoyager --on cloud

Results drift as the live sites change, so compare runs made close together.
"""

from __future__ import annotations

import json
import os
from typing import Any, Optional

import cua_bench as cb
from cua_bench.adapters import judges
from cua_bench.adapters.datasets import cache_dir, fetch_url
from cua_bench.adapters.web import LiveWebAdapter

COMMIT = "5a7896738c10bfb8b9edccce6bb0e0411f8ae569"
DATA_URL = f"https://raw.githubusercontent.com/MinorJerry/WebVoyager/{COMMIT}/data/WebVoyager_data.jsonl"
DATA_SHA256 = "69b19fd86c23f1a500244a3724e039aa7ca6a1223d03e11eb10e308d4f11c488"
JUDGE_MODEL = "gpt-4.1"


def load_rows(path: Optional[str] = None) -> list[dict]:
    path = path or str(fetch_url(DATA_URL, cache_dir("webvoyager") / COMMIT / "WebVoyager_data.jsonl",
                                 DATA_SHA256))
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


class WebVoyagerAdapter(LiveWebAdapter):
    id = "webvoyager"
    version = f"WebVoyager@{COMMIT[:7]}"

    def __init__(self, chat: Any = None, data: Optional[str] = None) -> None:
        super().__init__()
        self._chat, self._data = chat, data

    def load_tasks(self, split: str) -> list[cb.Task]:
        sites = {s.strip() for s in os.environ.get("CUA_BENCH_WEBVOYAGER_SITES", "").split(",") if s.strip()}
        return [
            cb.Task(description=row["ques"], task_id=f"webvoyager-{row['id']}",
                    metadata={"webvoyager_id": row["id"], "site": row["web_name"],
                              "start_url": row["web"]})
            for row in load_rows(self._data)
            if not sites or row["web_name"] in sites
        ]

    async def judge(self, task: cb.Task, evidence: dict) -> Optional[float]:
        chat = self._chat or judges.OpenAIChat(JUDGE_MODEL, temperature=0, seed=42, max_tokens=1000)
        return await judges.webvoyager(task.description, evidence["answer"], evidence["screenshots"], chat)


ADAPTER = WebVoyagerAdapter().register(globals())
