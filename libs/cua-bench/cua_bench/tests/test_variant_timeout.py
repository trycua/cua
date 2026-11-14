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
import logging
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
            for i in range(3)
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
    assert "TimeoutError" in results[0].error and "operation timed out" in results[0].error
    assert "timed out after" not in results[0].error
    assert results[0].retries == 1  # the inner TimeoutError stayed retryable
    assert len(sdk.calls) == 2  # one sandbox per try


async def test_sandbox_start_is_outside_the_variant_budget(monkeypatch, task_dir, tmp_path):
    """Sandbox start outliving the budget still completes: ``--retries`` owns start."""
    fake = FakeSDK(delay=6)
    monkeypatch.setattr(sandboxes, "_sdk", lambda: (fakes.FakeImage, fake.Sandbox))
    runner = BatchRunner(
        Target(on="local", concurrency=1),
        AgentOptions(oracle=True),
        variant_timeout_s=4,
    )
    results = await asyncio.wait_for(runner.run(_jobs(task_dir, tmp_path / "out", n=1)), timeout=30)
    assert results[0].status == "completed"


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


@pytest.mark.parametrize("bad", [-1, float("nan"), float("inf")])
def test_a_negative_or_nonfinite_variant_timeout_is_rejected(task_dir, tmp_path, bad):
    from cua_bench.cli.commands import run as run_cmd

    args = SimpleNamespace(
        dataset_path=str(task_dir.parent),
        task_filter=None,
        max_variants=None,
        attempts=1,
        retries=0,
        variant_timeout=bad,
    )
    with pytest.raises(ValueError, match="variant-timeout"):
        run_cmd.build_jobs(args, "r1", tmp_path / "out", Target(on="local"), "dataset")


@pytest.mark.parametrize("bad", [-1, float("nan"), float("inf")])
def test_batch_runner_rejects_a_negative_or_nonfinite_variant_timeout(bad):
    """The programmatic API gates the same values the CLI rejects."""
    with pytest.raises(ValueError, match="variant_timeout_s"):
        BatchRunner(
            Target(on="local"), AgentOptions(oracle=True), variant_timeout_s=bad
        )


def test_a_nonfinite_harness_budget_falls_back_to_the_default(monkeypatch):
    """``call_at(inf)`` crashes the event loop and nan never fires, so a
    non-finite env budget cannot reach the deadline."""
    from cua_bench.runner.batch_runner import (
        DEFAULT_VARIANT_TIMEOUT_S,
        _default_variant_timeout_s,
    )

    monkeypatch.setenv("CUA_BENCH_HARNESS_TIMEOUT", "inf")
    assert _default_variant_timeout_s() == DEFAULT_VARIANT_TIMEOUT_S
    monkeypatch.setenv("CUA_BENCH_HARNESS_TIMEOUT", "7200")
    assert _default_variant_timeout_s() == 7200 + 5400


# ── Bounded sandbox release ──────────────────────────────────────────────


def _wedged_sdk(monkeypatch, aexit_seen):
    """An SDK whose ephemeral sandbox hangs inside ``__aexit__`` forever."""

    class WedgedRelease:
        async def __aenter__(self):
            return object()

        async def __aexit__(self, *exc_info):
            aexit_seen.append(exc_info)
            await asyncio.Event().wait()
            return None

    class Sandbox:
        @classmethod
        def ephemeral(cls, image=None, **kwargs):
            return WedgedRelease()

    monkeypatch.setattr(sandboxes, "_sdk", lambda: (fakes.FakeImage, Sandbox))


async def test_a_wedged_sandbox_release_is_bounded(monkeypatch, tmp_path, caplog):
    """A release that never returns cannot re-stall the batch a frame later:
    the bound lets the batch proceed and warns the sandbox may leak."""
    aexit_seen: list[tuple] = []
    _wedged_sdk(monkeypatch, aexit_seen)
    monkeypatch.setattr(sandboxes, "TIME_TO_RELEASE_S", 0.05)
    progress = []
    spec = EnvSpec(provider="native", os_type="linux", kind="container", image=IMG)

    async def use():
        async with sandboxes.open_sandbox(
            spec, Target(on="local"), on_progress=progress.append
        ):
            pass

    with caplog.at_level(logging.WARNING, sandboxes.logger.name):
        await asyncio.wait_for(use(), timeout=10)
    assert aexit_seen == [(None, None, None)]
    assert "may leak" in caplog.text
    assert not any("released" in getattr(p, "message", "") for p in progress)


async def test_a_release_that_fails_on_its_own_is_not_swallowed(monkeypatch, tmp_path):
    """Only our deadline is swallowed: a TimeoutError ``__aexit__`` raises
    itself still propagates (the ``expired()`` discrimination)."""

    class TimeoutExit:
        async def __aenter__(self):
            return object()

        async def __aexit__(self, *exc_info):
            raise TimeoutError("sdk-internal release timeout")

    class Sandbox:
        @classmethod
        def ephemeral(cls, image=None, **kwargs):
            return TimeoutExit()

    monkeypatch.setattr(sandboxes, "_sdk", lambda: (fakes.FakeImage, Sandbox))
    monkeypatch.setattr(sandboxes, "TIME_TO_RELEASE_S", 30)
    spec = EnvSpec(provider="native", os_type="linux", kind="container", image=IMG)
    with pytest.raises(TimeoutError, match="sdk-internal"):
        async with sandboxes.open_sandbox(spec, Target(on="local")):
            pass


async def test_aexit_gets_the_real_in_flight_exception(monkeypatch, tmp_path):
    """``__aexit__`` sees the body's exception, not the caller's: a handled
    error outside the block used to leak in through ``sys.exc_info()``."""
    seen: list[tuple] = []

    class Recorder:
        async def __aenter__(self):
            return object()

        async def __aexit__(self, *exc_info):
            seen.append(exc_info)
            return None

    class Sandbox:
        @classmethod
        def ephemeral(cls, image=None, **kwargs):
            return Recorder()

    monkeypatch.setattr(sandboxes, "_sdk", lambda: (fakes.FakeImage, Sandbox))
    spec = EnvSpec(provider="native", os_type="linux", kind="container", image=IMG)

    try:
        raise KeyError("caller-context")
    except KeyError:
        async with sandboxes.open_sandbox(spec, Target(on="local")):
            pass
    assert seen == [(None, None, None)]

    with pytest.raises(ValueError, match="body"):
        async with sandboxes.open_sandbox(spec, Target(on="local")):
            raise ValueError("body")
    assert seen[-1][0] is ValueError
