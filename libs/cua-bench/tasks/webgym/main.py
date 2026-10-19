"""WebGym (test split) on cua-bench (bench-web image, live web), built on ``BenchAdapter``.

1,167 test tasks on live websites (microsoft/webgym, code MIT; tasks
microsoft/webgym_tasks on Hugging Face, CDLA-Permissive-2.0, pinned
revision). Setup opens the task's website; evaluate applies WebGym's
rubric judge (upstream prompts): every reference fact must be verified by
the screenshots and, when the agent answered, its answer must be supported
by them. Needs egress in the guest and ``OPENAI_API_KEY``.

::

    cb run dataset libs/cua-bench/tasks/webgym --agent <agent> --max-variants 10
    CUA_BENCH_WEBGYM_DIFFICULTY=1,2 cb run dataset libs/cua-bench/tasks/webgym --on cloud
"""

from __future__ import annotations

import json
import os
from typing import Any, Optional

import cua_bench as cb
from cua_bench.adapters import judges
from cua_bench.adapters.datasets import hf_file
from cua_bench.adapters.web import LiveWebAdapter

HF_REPO = "microsoft/webgym_tasks"
HF_REVISION = "a61330203480bea9b90b8e954ecf0b084a114cca"
TEST_SHA256 = "3553c072659323f328fb8a845273d28ca6f60443ae6d4e981acc5752a069edd1"
JUDGE_MODEL = "gpt-4.1"


def load_rows(path: Optional[str] = None) -> list[dict]:
    path = path or str(hf_file(HF_REPO, "test.jsonl", HF_REVISION, sha256=TEST_SHA256, name="webgym"))
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


class WebGymAdapter(LiveWebAdapter):
    id = "webgym"
    version = f"webgym_tasks@{HF_REVISION[:7]}"

    def __init__(self, chat: Any = None, data: Optional[str] = None) -> None:
        super().__init__()
        self._chat, self._data = chat, data

    def load_tasks(self, split: str) -> list[cb.Task]:
        wanted = {d.strip() for d in os.environ.get("CUA_BENCH_WEBGYM_DIFFICULTY", "").split(",") if d.strip()}
        return [
            cb.Task(description=row["task_name"], task_id=f"webgym-{row['task_id']}",
                    metadata={"webgym_id": row["task_id"], "start_url": row["website"],
                              "difficulty": row.get("difficulty"), "domain": row.get("domain"),
                              "reference": row.get("evaluator_reference") or [],
                              "definite_answer": row.get("definite_answer") or ""})
            for row in load_rows(self._data)
            if not wanted or str(row.get("difficulty")) in wanted
        ]

    async def judge(self, task: cb.Task, evidence: dict) -> Optional[float]:
        chat = self._chat or judges.OpenAIChat(JUDGE_MODEL, temperature=0)
        return await judges.webgym(task.description, task.metadata["reference"], evidence["answer"],
                                   evidence["actions"], evidence["screenshots"], chat)


ADAPTER = WebGymAdapter().register(globals())
