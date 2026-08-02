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
# Helpers
# ---------------------------------------------------------------------------

def _mock_session(netlist_content: str, components=None, compare_score: float = 1.0):
    session = MagicMock()
    session.apps.kicad.read_netlist = AsyncMock(return_value=netlist_content)
    session.apps.kicad.get_components = AsyncMock(return_value=components or [])
    session.apps.kicad.compare_netlist = AsyncMock(return_value=compare_score)
    return session


# ---------------------------------------------------------------------------
# Evaluate: missing / empty netlist
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_evaluate_returns_zero_when_netlist_missing():
    task = kicad_task.load()[0]
    session = _mock_session("")
    assert await kicad_task.evaluate(task, session) == [0.0]
    session.apps.kicad.compare_netlist.assert_not_called()


@pytest.mark.asyncio
async def test_evaluate_returns_zero_when_netlist_whitespace_only():
    task = kicad_task.load()[0]
    session = _mock_session("   \n  ")
    assert await kicad_task.evaluate(task, session) == [0.0]


# ---------------------------------------------------------------------------
# Evaluate: Path 1 — structural comparison via compare_netlist()
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_evaluate_structural_perfect_match():
    """compare_netlist returns 1.0 → evaluate returns [1.0]."""
    task = kicad_task.load()[0]
    session = _mock_session("(some netlist)", compare_score=1.0)
    scores = await kicad_task.evaluate(task, session)
    assert scores == [1.0]
    session.apps.kicad.compare_netlist.assert_called_once()
    session.apps.kicad.get_components.assert_not_called()


@pytest.mark.asyncio
async def test_evaluate_structural_partial_match():
    """compare_netlist returns partial score → evaluate passes it through."""
    task = kicad_task.load()[0]
    session = _mock_session("(some netlist)", compare_score=0.5)
    scores = await kicad_task.evaluate(task, session)
    assert scores == [0.5]


@pytest.mark.asyncio
async def test_evaluate_structural_passes_correct_paths():
    """compare_netlist is called with the right candidate and reference paths."""
    task = kicad_task.load()[0]
    session = _mock_session("(some netlist)", compare_score=1.0)
    await kicad_task.evaluate(task, session)
    call_kwargs = session.apps.kicad.compare_netlist.call_args.kwargs
    assert call_kwargs["candidate_path"] == kicad_task.NETLIST_PATH
    assert call_kwargs["reference_path"].endswith("kicad_twoleds_netlist.net")


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
    session = _mock_session("(export (version E)(components)(nets))", components)
    scores = await kicad_task.evaluate(task, session)
    assert scores == [1.0]
    session.apps.kicad.compare_netlist.assert_not_called()


@pytest.mark.asyncio
async def test_evaluate_component_count_wrong():
    """Wrong component counts → score < 1.0."""
    from cua_bench import Task
    task = Task(
        description="test",
        metadata={"expected_components": {"R": 2, "D": 2}},
    )
    components = [{"ref": "R1"}, {"ref": "D1"}, {"ref": "D2"}]  # only 1 R
    session = _mock_session("(export (version E)(components)(nets))", components)
    scores = await kicad_task.evaluate(task, session)
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
    session.apps.kicad.compare_netlist.assert_not_called()
    session.apps.kicad.get_components.assert_not_called()
