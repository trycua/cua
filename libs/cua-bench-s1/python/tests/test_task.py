"""Tests for the CuaTask/OptionSpec schema: round-trip serialization and
validation of malformed tasks."""
from __future__ import annotations

import pytest

from cua_bench_s1.task import CuaTask, OptionSpec, dataset_hash, save_jsonl, load_jsonl


def _make_task(task_id: str = "t1") -> CuaTask:
    return CuaTask(
        id=task_id,
        family="form_filling",
        app="demo_app",
        modality_available=["text", "multimodal"],
        screenshot="shot.png",
        ax_tree="# Demo\n- [el_0] Edit \"Email\" value=\"\"",
        ax_tree_source="synthetic",
        elements=[{"id": "el_0", "role": "Edit", "label": "Email", "frame": [0, 0, 10, 10]}],
        elements_source="synthetic_spec",
        entities=[{"id": "ent_0", "label": "Email", "value": "a@b.com"}],
        options=[
            OptionSpec("el_0", "Edit", "Email", "fill", "ent_0"),
            OptionSpec("el_0", "Edit", "Email", "skip"),
        ],
        expected={"el_0": "fill"},
    )


def test_valid_task_round_trips_through_serialization():
    task = _make_task()
    d = task.to_json()
    restored = CuaTask.from_json(d)
    assert restored == task
    assert isinstance(restored.options[0], OptionSpec)


def test_round_trip_through_jsonl(tmp_path):
    tasks = [_make_task("t1"), _make_task("t2")]
    path = tmp_path / "tasks.jsonl"
    save_jsonl(tasks, path)
    restored = load_jsonl(path)
    assert restored == tasks


def test_multimodal_without_screenshot_is_rejected():
    with pytest.raises(ValueError):
        CuaTask(
            id="bad",
            family="form_filling",
            app="demo_app",
            modality_available=["multimodal"],
            screenshot=None,
            ax_tree=None,
            ax_tree_source=None,
            elements=[],
            elements_source="synthetic_spec",
            entities=[],
            options=[],
            expected={},
        )


def test_text_without_ax_tree_is_rejected():
    with pytest.raises(ValueError):
        CuaTask(
            id="bad",
            family="form_filling",
            app="demo_app",
            modality_available=["text"],
            screenshot=None,
            ax_tree=None,
            ax_tree_source=None,
            elements=[],
            elements_source="synthetic_spec",
            entities=[],
            options=[],
            expected={},
        )


def test_dataset_hash_is_stable_and_order_independent():
    tasks = [_make_task("t1"), _make_task("t2")]
    h1 = dataset_hash(tasks)
    h2 = dataset_hash(list(reversed(tasks)))
    assert h1 == h2

    tasks[0].provenance["note"] = "changed"
    h3 = dataset_hash(tasks)
    assert h3 != h1
