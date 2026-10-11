"""One sandbox lifecycle: the gym API (make/reset/evaluate/close) and the
programmatic runners open sandboxes exactly like ``cb run`` does
(``sandboxes.open_sandbox``), and release them on close. Hermetic."""

from __future__ import annotations

from pathlib import Path

import pytest
from cua_bench import make, run_single_task, sandboxes
from cua_bench.targets import Target

from .fakes import FakeSDK

HELLO = Path(__file__).resolve().parents[2] / "example_tasks" / "hello_file_env"


@pytest.fixture
def sdk(monkeypatch):
    fake = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", fake.pair)
    for var in ("CUA_BENCH_ON", "CUA_BENCH_RUNTIME", "CUA_BENCH_IMAGE"):
        monkeypatch.delenv(var, raising=False)
    return fake


@pytest.mark.asyncio
async def test_gym_reset_opens_and_close_releases(sdk):
    env = make(str(HELLO))
    _, task = await env.reset(task_id=1)
    assert task.metadata["word"] == "bench"
    (call,) = sdk.calls
    assert call["on"] == "local" and call["image"].ref.startswith("ghcr.io/trycua/linux")
    assert sdk.live == 1
    await env.solve()
    assert await env.evaluate() == [1.0]
    await env.close()
    assert sdk.live == 0 and sdk.released == 1


@pytest.mark.asyncio
async def test_gym_target_cloud(sdk):
    env = make(str(HELLO))
    env.target = Target(on="cloud")
    await env.reset(task_id=0)
    assert sdk.calls[0]["on"] == "cloud" and sdk.calls[0]["max_pool_size"] == 1
    await env.close()
    assert sdk.live == 0


@pytest.mark.asyncio
async def test_reset_twice_releases_the_first_sandbox(sdk):
    env = make(str(HELLO))
    await env.reset(task_id=0)
    env.current_task = None
    await env.reset(task_id=1)
    assert sdk.released == 1 and sdk.live == 1
    await env.close()
    assert sdk.live == 0


@pytest.mark.asyncio
async def test_run_single_task_oracle(sdk):
    result = await run_single_task(HELLO, task_index=0, oracle=True)
    assert result.success and result.reward == 1.0
    assert sdk.live == 0 and sdk.released == 1


@pytest.mark.asyncio
async def test_starter_template_is_a_container_task_that_passes(sdk):
    """`cb task create` scaffolds this: a native Linux container task."""
    from cua_bench.targets import resolve_env_spec, resolve_target

    template = Path(__file__).resolve().parents[1] / "templates" / "starter_env"
    env = make(str(template))
    tasks = env.tasks_config_fn()
    spec = resolve_env_spec(tasks[0].computer, resolve_target(environ={}))
    assert (spec.provider, spec.os_type, spec.kind) == ("native", "linux", "container")
    for index in range(len(tasks)):
        result = await run_single_task(template, task_index=index, oracle=True)
        assert result.success and result.reward == 1.0


def test_core_interact_runs_setup_and_evaluate(sdk, monkeypatch, capsys):
    """cb.interact awaited nothing before 0.3 (env.reset is async)."""
    import builtins

    import cua_bench

    monkeypatch.setenv("CUA_BENCH_NO_BANNER", "1")
    monkeypatch.setattr(builtins, "input", lambda *a: "")
    cua_bench.interact(str(HELLO), task_id=0)
    out = capsys.readouterr().out
    assert "Setup complete" in out and "Evaluation result: [0.0]" in out
    assert sdk.live == 0 and sdk.released == 1
