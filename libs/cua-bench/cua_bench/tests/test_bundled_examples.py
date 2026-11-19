"""Regression tests for bundled example task definitions."""

import importlib.util
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

EXAMPLES = Path(__file__).parents[2] / "example_tasks"
TASK_PATH = EXAMPLES / "2048_env" / "main.py"


@pytest.fixture
def module_2048():
    spec = importlib.util.spec_from_file_location("test_2048", TASK_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_bundled_examples_declare_native_tasks():
    """The simulated provider is gone: every bundled example runs in a sandbox."""
    for main_py in sorted(EXAMPLES.glob("*/main.py")):
        text = main_py.read_text()
        assert '"simulated"' not in text and '"webtop"' not in text, main_py


@pytest.mark.asyncio
async def test_2048_setup_awaits_window_launch(module_2048):
    session = SimpleNamespace(launch_window=AsyncMock(return_value=42))

    await module_2048.start(None, session)

    session.launch_window.assert_awaited_once()
    assert module_2048.pid == 42


@pytest.mark.asyncio
async def test_2048_evaluate_awaits_max_tile_query(module_2048):
    module_2048.pid = 42
    session = SimpleNamespace(execute_javascript=AsyncMock(return_value=128))

    result = await module_2048.evaluate(None, session)

    session.execute_javascript.assert_awaited_once_with(42, "window.__max_tile || 0")
    assert result == [0.0625]


@pytest.mark.asyncio
async def test_2048_solver_awaits_queries_and_actions(module_2048):
    module_2048.pid = 42
    session = SimpleNamespace(
        execute_action=AsyncMock(),
        execute_javascript=AsyncMock(side_effect=[False, "left", True]),
    )

    await module_2048.solve(None, session)

    assert session.execute_javascript.await_count == 3
    session.execute_action.assert_awaited_once()
    assert session.execute_action.await_args.args[0].key == "left"
