"""Tests for KiCad-task: task loading and evaluate logic with mock session."""

import importlib.util
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

import pytest

# Load task from sibling main.py
_MAIN = Path(__file__).resolve().parent / "main.py"
spec = importlib.util.spec_from_file_location("kicad_task", _MAIN)
kicad_task = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kicad_task)

_TASK_DIR = Path(__file__).resolve().parent
_REFERENCE_NETLIST = (_TASK_DIR / "kicad_twoleds_netlist.net").read_text()


# ---------------------------------------------------------------------------
# Task loading
# ---------------------------------------------------------------------------

def test_load_returns_correct_count():
    tasks = kicad_task.load()
    assert len(tasks) == len(kicad_task.TASK_VARIANTS)


def test_load_led_circuit_variant():
    task = kicad_task.load()[0]
    assert task.metadata["circuit_name"] == "LEDCircuit"
    assert task.metadata.get("reference_netlist") == "kicad_twoleds_netlist.net"


def test_load_empty_netlist_variant():
    task = kicad_task.load()[1]
    assert task.metadata["circuit_name"] == "EmptyNetlistExport"
    assert task.metadata.get("netlist_export_only") is True
    assert not task.metadata.get("reference_netlist")


# ---------------------------------------------------------------------------
# Evaluate: missing / empty netlist
# ---------------------------------------------------------------------------

def _mock_session(netlist_content: str, components=None):
    session = MagicMock()
    session.apps.kicad.read_netlist = AsyncMock(return_value=netlist_content)
    session.apps.kicad.get_components = AsyncMock(return_value=components or [])
    return session


@pytest.mark.asyncio
async def test_evaluate_returns_zero_when_netlist_missing():
    task = kicad_task.load()[0]
    session = _mock_session("")
    assert await kicad_task.evaluate(task, session) == [0.0]


@pytest.mark.asyncio
async def test_evaluate_returns_zero_when_netlist_whitespace_only():
    task = kicad_task.load()[0]
    session = _mock_session("   \n  ")
    assert await kicad_task.evaluate(task, session) == [0.0]


# ---------------------------------------------------------------------------
# Evaluate: Path 1 — structural comparison via reference_netlist
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_evaluate_structural_perfect_match():
    """Agent output identical to reference → score 1.0."""
    task = kicad_task.load()[0]
    session = _mock_session(_REFERENCE_NETLIST)
    scores = await kicad_task.evaluate(task, session)
    assert scores == [1.0]
    # get_components should NOT be called when using reference netlist
    session.apps.kicad.get_components.assert_not_called()


@pytest.mark.asyncio
async def test_evaluate_structural_wrong_netlist():
    """Agent output with wrong components → score < 1.0."""
    task = kicad_task.load()[0]
    wrong_netlist = "(export (version E)(components)(nets))"
    session = _mock_session(wrong_netlist)
    scores = await kicad_task.evaluate(task, session)
    assert len(scores) == 1
    assert scores[0] < 1.0


# ---------------------------------------------------------------------------
# Evaluate: Path 2 — expected_components fallback
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_evaluate_component_count_correct():
    """Correct component counts → score 1.0."""
    from cua_bench import Task
    task = Task(
        description="test",
        metadata={"expected_components": {"R": 1, "D": 2}},
    )
    components = [{"ref": "R1"}, {"ref": "D1"}, {"ref": "D2"}]
    session = _mock_session("(export (version E)(components (comp (ref R1)))(nets))", components)
    scores = await kicad_task.evaluate(task, session)
    assert scores == [1.0]


@pytest.mark.asyncio
async def test_evaluate_component_count_wrong():
    """Wrong component counts → score 0.0."""
    from cua_bench import Task
    task = Task(
        description="test",
        metadata={"expected_components": {"R": 2, "D": 2}},
    )
    # Only 1 R instead of 2
    components = [{"ref": "R1"}, {"ref": "D1"}, {"ref": "D2"}]
    session = _mock_session("(export (version E)(components (comp (ref R1)))(nets))", components)
    scores = await kicad_task.evaluate(task, session)
    assert len(scores) == 1
    assert scores[0] < 1.0


# ---------------------------------------------------------------------------
# Evaluate: Path 3 — netlist_export_only (any valid .net passes)
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_evaluate_export_only_passes_with_valid_netlist():
    task = kicad_task.load()[1]  # EmptyNetlistExport
    session = _mock_session("(export (version E)(components)(nets))")
    scores = await kicad_task.evaluate(task, session)
    assert scores == [1.0]
    session.apps.kicad.get_components.assert_not_called()
