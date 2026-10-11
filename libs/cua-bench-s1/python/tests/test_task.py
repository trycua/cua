"""Tests for the CuaTask/OptionSpec schema: round-trip serialization and
validation of malformed tasks."""
from __future__ import annotations

import os

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


def test_goal_text_reads_each_source_and_never_double_states_a_rendered_goal():
    from cua_bench_s1.task import goal_text

    t = _make_task()
    assert goal_text(t) is None and t.goal is None

    # GUI-360: episode request + a step subtask that adds information.
    t.provenance = {"request": "Add the Draw tab to the ribbon", "subtask": "Open File > Options"}
    assert t.goal == "Add the Draw tab to the ribbon\nCurrent step: Open File > Options"

    # A step instruction that merely repeats the episode goal is stated once.
    t.provenance = {"request": "Add the Draw tab", "subtask": "Add the Draw tab"}
    assert t.goal == "Add the Draw tab"

    # AndroidControl's own key pair.
    t.provenance = {"goal": "Book a table", "step_instruction": "Tap Reserve"}
    assert t.goal == "Book a table\nCurrent step: Tap Reserve"

    # A synthetic task renders its goal into the observation, so a prompt
    # builder must NOT prepend it again -- that invariant is `goal_in_state`.
    t.provenance = {"synthetic_goal": "Sign in with the credentials below", "goal_in_state": True}
    assert t.goal is None
    assert t.provenance["synthetic_goal"] == "Sign in with the credentials below"


def test_content_key_ignores_identity_but_not_the_decision():
    from cua_bench_s1.task import content_key

    a, b = _make_task("t1"), _make_task("t2")
    # Different ids, different seeds, same decision -> same content key. This is
    # the case `(app_id, index)` bucketing misses and that put the same screen
    # in both train and test.
    a.provenance = {"seed": 1}
    b.provenance = {"seed": 999}
    assert content_key(a) == content_key(b)

    b.expected = {"el_0": "click"}
    assert content_key(a) != content_key(b)


def test_content_deduper_rejects_a_duplicate_across_splits():
    from cua_bench_s1.task import ContentDeduper

    d = ContentDeduper()
    a, b = _make_task("t1"), _make_task("t2")
    assert d.accept(a, "train") is True
    # Same content, different split: rejected, and it can say where it landed.
    assert d.accept(b, "test") is False
    assert d.placed_split(b) == "train"
    assert d.rejected == 1
    assert len(d) == 1


def test_generate_dataset_emits_no_duplicated_content_across_splits(tmp_path):
    """The end-to-end guarantee: no task's content may appear in two splits,
    and none may be duplicated within one."""
    from cua_bench_s1.datagen.generator import generate_dataset
    from cua_bench_s1.datagen.specs import APPS_BY_FAMILY
    from cua_bench_s1.task import content_key, stable_digest

    def split_fn(app_id, i):
        return ("train", "val", "test")[stable_digest(app_id, i) % 3]

    apps = APPS_BY_FAMILY["consent_checkbox"] + APPS_BY_FAMILY["pagination"]
    tasks = generate_dataset(apps, n_per_app=25, seed=2026, modality_available=("text",),
                             out_dir=tmp_path, split_fn=split_fn)
    assert tasks, "generated nothing"

    by_split: dict[str, list[str]] = {}
    for t in tasks:
        by_split.setdefault(t.provenance["split"], []).append(content_key(t))

    keys = [k for v in by_split.values() for k in v]
    assert len(keys) == len(set(keys)), "duplicate task content within the dataset"
    for s1, k1 in by_split.items():
        for s2, k2 in by_split.items():
            if s1 < s2:
                assert not (set(k1) & set(k2)), f"content leaked between {s1} and {s2}"

    # The bucket lives in provenance, not in the public/private `split` field.
    assert all(t.split == "public" for t in tasks)


def test_resolve_screenshot_uses_the_dataset_root_without_rewriting_the_task(tmp_path):
    from cua_bench_s1.task import dataset_hash, resolve_screenshot

    shots = tmp_path / "screenshots"
    shots.mkdir()
    (shots / "a.png").write_bytes(b"not-really-a-png")

    t = _make_task("t1")
    t.modality_available = ["multimodal"]
    t.screenshot = "screenshots/a.png"
    before = dataset_hash([t])

    assert resolve_screenshot(t, tmp_path) == tmp_path / "screenshots" / "a.png"
    # Resolution must not mutate the stored path: it is hashed content, so
    # absolutizing it on load would change every screenshot-bearing split.
    assert t.screenshot == "screenshots/a.png"
    assert dataset_hash([t]) == before


def test_stable_digest_is_identical_in_a_fresh_interpreter():
    """`stable_digest` must not depend on PYTHONHASHSEED, which is what the
    built-in `hash()` does for strings. Anything seeding a generator or
    bucketing a split off `hash()` silently produces a different dataset every
    run, so this is checked in a real subprocess under a different salt rather
    than in-process (where the salt is fixed for this interpreter's lifetime).
    """
    import subprocess
    import sys

    from cua_bench_s1.task import stable_digest

    expected = stable_digest("clinic_intake", 7)
    prog = (
        "from cua_bench_s1.task import stable_digest;"
        "print(stable_digest('clinic_intake', 7))"
    )
    for salt in ("0", "1", "12345"):
        env = {**os.environ, "PYTHONHASHSEED": salt}
        out = subprocess.run([sys.executable, "-c", prog], capture_output=True, text=True,
                             check=True, env=env).stdout.strip()
        assert int(out) == expected, f"stable_digest changed under PYTHONHASHSEED={salt}"


def test_generated_dataset_hash_reproduces_across_processes(tmp_path):
    """The pre-registration guarantee: regenerating a split from the same
    inputs must reproduce its `dataset_hash` byte for byte. A `uuid.uuid4()`
    task id or a `hash()`-derived per-task seed breaks this, and then a changed
    eval set is indistinguishable from an unchanged one because every
    regeneration looks like a change."""
    import json
    import subprocess
    import sys

    prog = (
        "import json, sys, tempfile;"
        "from pathlib import Path;"
        "from cua_bench_s1.datagen.generator import generate_dataset;"
        "from cua_bench_s1.datagen.specs import APPS_BY_FAMILY;"
        "from cua_bench_s1.task import dataset_hash;"
        "d=tempfile.mkdtemp();"
        "t=generate_dataset(APPS_BY_FAMILY['form_filling'], 3, 2026, ('text',), Path(d));"
        "print(json.dumps({'hash': dataset_hash(t), 'ids': [x.id for x in t]}))"
    )
    seen = []
    for salt in ("0", "7", "999"):
        env = {**os.environ, "PYTHONHASHSEED": salt}
        out = subprocess.run([sys.executable, "-c", prog], capture_output=True, text=True,
                             check=True, env=env).stdout.strip().splitlines()[-1]
        seen.append(json.loads(out))

    assert seen[0]["ids"] == seen[1]["ids"] == seen[2]["ids"], "task ids are not deterministic"
    assert seen[0]["hash"] == seen[1]["hash"] == seen[2]["hash"], (
        f"dataset_hash is not reproducible across processes: {[s['hash'] for s in seen]}")
