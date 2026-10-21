"""``--variant-timeout``: a bound on one variant's setup, agent and evaluation.

trycua/cua#5193: BatchRunner bounded nothing, so one sandbox call that never
returned held ``asyncio.gather`` forever: ``cb run`` never wrote summary.json
and the hung variant's sandbox stayed up. ``BatchRunner(variant_timeout_s=)``
cancels the variant the way Ctrl-C does (the same ``async with`` releases its
sandbox), records it failed, and does not retry it. Sandbox start stays
unbounded: ``--retries`` already covers it.
"""

from __future__ import annotations

import asyncio
import json
import textwrap
from types import SimpleNamespace

import pytest
from cua_bench import sandboxes
from cua_bench.runner import AgentOptions, BatchRunner, Job
from cua_bench.targets import EnvSpec, Target

from . import fakes
from .fakes import FakeSDK

IMG = "registry.example/desktop:docker-1"

TASK = textwrap.dedent(
    """
    import cua_bench as cb

    @cb.tasks_config(split="train")
    def load():
        return [
            cb.Task(description=f"variant {i}", metadata={"i": i},
                    computer={"provider": "native", "setup_config": {"os_type": "linux",
                              "image": "%s"}})
            for i in range(2)
        ]

    @cb.solve_task(split="train")
    async def solve(task_cfg, session):
        if task_cfg.metadata["i"] == 1:
            # A shell call that never returns while the sandbox stays healthy.
            await session.run_command("sleep 100000")
        else:
            await session.write_file("/tmp/answer", "done")

    @cb.evaluate_task(split="train")
    async def evaluate(task_cfg, session):
        if task_cfg.metadata["i"] == 2:
            await session.run_command("answer-with-timeout-error")
        return [1.0]
    """
    % IMG
)


@pytest.fixture
def task_dir(tmp_path):
    path = tmp_path / "hang_env"
    path.mkdir()
    (path / "main.py").write_text(TASK)
    return path


@pytest.fixture
def sdk(monkeypatch):
    """The in-memory SDK; ``sleep 100000`` never returns, ``answer-with-timeout-error`` raises."""
    fake = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", lambda: (fakes.FakeImage, fake.Sandbox))
    original_run = fakes.FakeShell.run

    async def run(self, command, timeout=None, background=False):
        if "sleep 100000" in command:
            self.commands.append((command, timeout))
            await asyncio.Event().wait()  # never returns
        if "answer-with-timeout-error" in command:
            raise TimeoutError("operation timed out")
        return await original_run(self, command, timeout=timeout, background=background)

    monkeypatch.setattr(fakes.FakeShell, "run", run)
    return fake


def _jobs(task_dir, out, n=2):
    spec = EnvSpec(provider="native", os_type="linux", kind="container", image=IMG)
    return [
        Job(
            task_path=task_dir,
            variant=i,
            spec=spec,
            session_id=f"s{i}",
            output_dir=out / f"hang_env_v{i}",
        )
        for i in range(n)
    ]


async def test_a_hung_variant_times_out_and_the_batch_finishes(sdk, task_dir, tmp_path):
    runner = BatchRunner(
        Target(on="local", concurrency=2),
        AgentOptions(oracle=True),
        retries=2,
        retry_backoff_s=0,
        variant_timeout_s=8,
    )
    # The watchdog keeps the test bounded: unfixed code stalls gather forever.
    results = await asyncio.wait_for(runner.run(_jobs(task_dir, tmp_path / "out")), timeout=30)

    by_variant = {r.variant: r for r in results}
    sibling = by_variant[0]
    assert sibling.status == "completed" and sibling.reward == 1.0

    hung = by_variant[1]
    assert hung.status == "failed"
    assert hung.error == "variant timed out after 8 s"
    saved = json.loads((tmp_path / "out" / "hang_env_v1" / "result.json").read_text())
    assert saved["status"] == "failed" and "timed out" in saved["error"]

    # Cancelled like Ctrl-C: the async-with released its sandbox, once.
    assert sdk.live == 0 and sdk.released == 2
    # A timeout is not an infrastructure failure: no retry, no second sandbox.
    assert hung.retries == 0 and len(sdk.calls) == 2


async def test_a_timeout_error_inside_the_variant_still_retries(sdk, task_dir, tmp_path):
    """A TimeoutError the variant raised itself is not the deadline firing."""
    runner = BatchRunner(
        Target(on="local", concurrency=2),
        AgentOptions(oracle=True),
        retries=1,
        retry_backoff_s=0,
        variant_timeout_s=8,
    )
    spec = EnvSpec(provider="native", os_type="linux", kind="container", image=IMG)
    job = Job(
        task_path=task_dir,
        variant=2,
        spec=spec,
        session_id="s2",
        output_dir=tmp_path / "out" / "hang_env_v2",
    )
    results = await asyncio.wait_for(runner.run([job]), timeout=30)

    assert results[0].status == "failed"
    assert "timed out after" not in results[0].error
    assert results[0].retries == 1  # the inner TimeoutError stayed retryable
    assert len(sdk.calls) == 2  # one sandbox per try


async def test_zero_variant_timeout_runs_unbounded(sdk, task_dir, tmp_path):
    runner = BatchRunner(
        Target(on="local", concurrency=2), AgentOptions(oracle=True), variant_timeout_s=0
    )
    results = await asyncio.wait_for(runner.run(_jobs(task_dir, tmp_path / "out", n=1)), timeout=30)
    assert results[0].status == "completed"


def test_variant_timeout_flag_parses_for_task_and_dataset():
    from cua_bench.cli.main import build_parser

    parser = build_parser()
    for sub in ("task", "dataset"):
        args = parser.parse_args(["run", sub, "path", "--variant-timeout", "120"])
        assert args.variant_timeout == 120.0
    args = parser.parse_args(["run", "task", "path"])
    assert args.variant_timeout is None


def test_a_negative_variant_timeout_is_rejected(task_dir, tmp_path):
    from cua_bench.cli.commands import run as run_cmd

    args = SimpleNamespace(
        dataset_path=str(task_dir.parent),
        task_filter=None,
        max_variants=None,
        attempts=1,
        retries=0,
        variant_timeout=-1,
    )
    with pytest.raises(ValueError, match="variant-timeout"):
        run_cmd.build_jobs(args, "r1", tmp_path / "out", Target(on="local"), "dataset")
