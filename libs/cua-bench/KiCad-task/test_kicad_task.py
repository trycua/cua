"""Simple tests for KiCad-task: load task and evaluate logic with mock session."""

import asyncio
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

import pytest

# Load task from sibling main.py
import importlib.util
_MAIN = Path(__file__).resolve().parent / "main.py"
spec = importlib.util.spec_from_file_location("kicad_task", _MAIN)
kicad_task = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kicad_task)


def test_load_returns_two_tasks():
    """Task config should return 2 variants."""
    tasks = kicad_task.load()
    assert len(tasks) == 2
    assert tasks[0].metadata.get("project_name") == "MyFirstBoard"
    assert tasks[1].metadata.get("project_name") == "BlinkyPCB"


@pytest.mark.asyncio
async def test_evaluate_fails_when_project_file_missing():
    """Evaluate returns 0.0 when project file does not exist (NOT_FOUND)."""
    task = kicad_task.load()[0]
    session = MagicMock()
    session.os_type = "linux"
    session.run_command = AsyncMock(return_value={"stdout": "NOT_FOUND", "stderr": "", "return_code": 0})

    score = await kicad_task.evaluate(task, session)
    assert score == [0.0]
    session.run_command.assert_called_once()


@pytest.mark.asyncio
async def test_evaluate_succeeds_when_project_file_exists():
    """Evaluate returns 1.0 when project file exists (FOUND)."""
    task = kicad_task.load()[0]
    session = MagicMock()
    session.os_type = "linux"
    session.run_command = AsyncMock(return_value={"stdout": "FOUND", "stderr": "", "return_code": 0})

    score = await kicad_task.evaluate(task, session)
    assert score == [1.0]
    session.run_command.assert_called_once()


@pytest.mark.asyncio
async def test_evaluate_windows_path_when_os_type_windows():
    """Evaluate uses Windows path and if exist when session is Windows."""
    task = kicad_task.load()[0]
    session = MagicMock()
    session.os_type = "windows"
    session.run_command = AsyncMock(return_value={"stdout": "FOUND", "stderr": "", "return_code": 0})

    score = await kicad_task.evaluate(task, session)
    assert score == [1.0]
    call_args = session.run_command.call_args[0][0]
    assert "if exist" in call_args
    assert "KiCadProjects" in call_args
    assert ".kicad_pro" in call_args
