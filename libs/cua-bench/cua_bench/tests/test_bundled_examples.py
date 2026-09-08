"""Regression tests for bundled example task definitions."""

import importlib.util
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

TASK_PATH = Path(__file__).parents[2] / "example_tasks" / "2048_env_simulated" / "main.py"


@pytest.fixture
def simulated_2048_module():
    spec = importlib.util.spec_from_file_location("test_simulated_2048", TASK_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.asyncio
async def test_simulated_2048_setup_awaits_window_launch(simulated_2048_module):
    session = SimpleNamespace(launch_window=AsyncMock(return_value=42))

    await simulated_2048_module.start(None, session)

    session.launch_window.assert_awaited_once()
    assert simulated_2048_module.pid == 42


@pytest.mark.asyncio
async def test_simulated_2048_evaluate_awaits_max_tile_query(simulated_2048_module):
    simulated_2048_module.pid = 42
    session = SimpleNamespace(execute_javascript=AsyncMock(return_value=128))

    result = await simulated_2048_module.evaluate(None, session)

    session.execute_javascript.assert_awaited_once_with(42, "window.__max_tile || 0")
    assert result == [0.0625]


@pytest.mark.asyncio
async def test_simulated_2048_solver_awaits_queries_and_steps(simulated_2048_module):
    simulated_2048_module.pid = 42
    step = AsyncMock()
    session = SimpleNamespace(
        env=SimpleNamespace(step=step),
        execute_javascript=AsyncMock(side_effect=[False, "left", True]),
    )

    await simulated_2048_module.solve(None, session)

    assert session.execute_javascript.await_count == 3
    step.assert_awaited_once()
    assert step.await_args.args[0].key == "left"
