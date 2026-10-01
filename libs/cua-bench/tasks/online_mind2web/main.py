"""Online-Mind2Web on cua-bench (bench-web image, live web), built on ``BenchAdapter``.

300 tasks on live websites (OSU-NLP-Group/Online-Mind2Web; code MIT, data
CC-BY-4.0 on Hugging Face, GATED: request access at
https://huggingface.co/datasets/osunlp/Online-Mind2Web, then set HF_TOKEN
or log in with the hf CLI). Setup opens the task's website; evaluate runs
WebJudge (upstream prompts and rule: key points, per-snapshot relevance,
final status) over the final screenshots and the agent's action history.
Needs egress in the guest, ``OPENAI_API_KEY`` and the dataset access.

::

    cb run dataset libs/cua-bench/tasks/online_mind2web --agent <agent> --max-variants 10
    CUA_BENCH_OM2W_LEVEL=easy cb run dataset libs/cua-bench/tasks/online_mind2web --on cloud
"""

from __future__ import annotations

import json
import os
from typing import Any, Optional

import cua_bench as cb
from cua_bench.adapters import judges
from cua_bench.adapters.datasets import hf_file
from cua_bench.adapters.web import LiveWebAdapter

CODE_COMMIT = "f0d805ee0e9e0b3ea70911e45e5264b72968f3dc"
HF_REPO = "osunlp/Online-Mind2Web"
HF_REVISION = "eacad896a84dc5b65e29b0b06e4699ab0544d701"
HF_FILE = "Online_Mind2Web.json"
JUDGE_MODEL = "o4-mini"


def load_rows(path: Optional[str] = None) -> list[dict]:
    path = path or str(hf_file(HF_REPO, HF_FILE, HF_REVISION, name="online-mind2web", gated=True))
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    return data if isinstance(data, list) else list(data.values())


class OnlineMind2WebAdapter(LiveWebAdapter):
    id = "online-mind2web"
    version = f"Online-Mind2Web@{HF_REVISION[:7]}"
    requires = frozenset({"egress", "openai", "hf-gated"})

    def __init__(self, chat: Any = None, data: Optional[str] = None) -> None:
        super().__init__()
        self._chat, self._data = chat, data

    def load_tasks(self, split: str) -> list[cb.Task]:
        level = os.environ.get("CUA_BENCH_OM2W_LEVEL", "").strip().lower()
        out = []
        for row in load_rows(self._data):
            if level and str(row.get("level", "")).lower() != level:
                continue
            text = row.get("confirmed_task") or row.get("task")
            out.append(cb.Task(description=text, task_id=f"om2w-{row['task_id']}",
                               metadata={"om2w_id": row["task_id"], "level": row.get("level"),
                                         "start_url": row["website"]}))
        return out

    async def judge(self, task: cb.Task, evidence: dict) -> Optional[float]:
        chat = self._chat or judges.OpenAIChat(JUDGE_MODEL)
        return await judges.webjudge(task.description, evidence["actions"], evidence["screenshots"], chat)


ADAPTER = OnlineMind2WebAdapter().register(globals())
