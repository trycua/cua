"""The ``dataset`` provider: tasks with no environment (hermetic)."""

from __future__ import annotations

import asyncio
import io
import json
import textwrap

import pytest
from cua_bench.computers.dataset import DatasetSession
from cua_bench.runner import AgentOptions, BatchRunner, Job
from cua_bench.targets import Target, check_requirements, plan_claims, resolve_env_spec
from cua_bench.types import ClickAction, DoneAction


def _png(w=40, h=30) -> bytes:
    from PIL import Image

    buf = io.BytesIO()
    Image.new("RGB", (w, h), (1, 2, 3)).save(buf, format="PNG")
    return buf.getvalue()


def test_env_spec_needs_no_sandbox():
    comp = {"provider": "dataset", "setup_config": {"requires": ["kvm", "env:X"], "width": 5}}
    spec = resolve_env_spec(comp, Target(on="cloud", kind="vm", image="ghcr.io/x/y:1"))
    assert spec.provider == "dataset" and not spec.needs_sandbox
    assert spec.kind == "none" and spec.image is None
    assert spec.backend("cloud") == "none" and spec.image_variant == "none"
    assert spec.requires == ("env:X",)  # kvm never applies without a VM
    assert plan_claims([("a", spec)], 4) == []
    with pytest.raises(Exception, match="X"):
        check_requirements([spec], Target(), environ={})


def test_unknown_provider_mentions_dataset():
    with pytest.raises(Exception, match="dataset"):
        resolve_env_spec({"provider": "bogus"}, Target())


def test_dataset_session_records_actions():
    s = DatasetSession()
    png = _png()
    s.show(png)
    assert (s.width, s.height) == (40, 30)
    assert asyncio.run(s.screenshot()) == png

    async def act():
        await s.execute_action(ClickAction(x=3, y=4))
        await s.execute_action(DoneAction())
        await s.report_infeasible("no such button")

    asyncio.run(act())
    assert s.actions[0] == {"type": "click", "x": 3, "y": 4}
    assert s.actions[1]["type"] == "done"
    assert s.points == [(3, 4)] and s.infeasible
    with pytest.raises(NotImplementedError, match="no environment"):
        s.shell  # noqa: B018 - anything a live desktop has


TASK = textwrap.dedent(
    """
    import io
    import cua_bench as cb
    from PIL import Image

    @cb.tasks_config(split="train")
    def load():
        return [cb.Task(description="click", metadata={"box": [10, 10, 20, 20]},
                        computer={"provider": "dataset"})]

    @cb.setup_task(split="train")
    async def start(task, session):
        buf = io.BytesIO(); Image.new("RGB", (64, 48)).save(buf, format="PNG")
        session.show(buf.getvalue())

    @cb.solve_task(split="train")
    async def solve(task, session):
        await session.click(15, 15)

    @cb.evaluate_task(split="train")
    async def evaluate(task, session):
        x0, y0, x1, y1 = task.metadata["box"]
        return [1.0 if any(x0 <= x <= x1 and y0 <= y <= y1 for x, y in session.points) else 0.0]
    """
)


def test_runner_runs_dataset_tasks_without_a_sandbox(tmp_path):
    task = tmp_path / "grounding"
    task.mkdir()
    (task / "main.py").write_text(TASK)
    spec = resolve_env_spec({"provider": "dataset"}, Target(on="cloud"))

    def opener(*a, **k):  # must never be called
        raise AssertionError("dataset tasks must not open a sandbox")

    runner = BatchRunner(Target(on="cloud"), AgentOptions(oracle=True), opener=opener)
    job = Job(task_path=task, variant=0, spec=spec, session_id="s0", output_dir=tmp_path / "o")
    (result,) = asyncio.run(runner.run([job]))
    assert result.status == "completed", result.error
    assert result.reward == 1.0
    assert result.kind == "none" and result.image_digest is None and result.backend == "none"
    data = json.loads((tmp_path / "o" / "result.json").read_text())
    assert data["image_variant"] == "none"


def test_extra_ports_are_exposed():
    from cua_bench.sandboxes import build_image

    comp = {"provider": "native", "setup_config": {"image": "ghcr.io/x/y:1", "server_port": 5000,
                                                    "ports": [9222, 8080, 5000]}}
    spec = resolve_env_spec(comp, Target())
    assert spec.ports == (8080, 9222)  # server_port is exposed on its own
    exposed = []

    class Img:
        @classmethod
        def from_registry(cls, ref, os_type, kind):
            return cls()

        def expose(self, port):
            exposed.append(port)
            return self

    build_image(spec, Img)
    assert exposed == [8080, 9222]
    with pytest.raises(Exception, match="ports"):
        resolve_env_spec({"setup_config": {"ports": ["x"]}}, Target())
