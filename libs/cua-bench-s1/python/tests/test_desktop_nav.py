"""Tests for the synthetic desktop command-navigation generator: schema
conformance, exactly one gold click, a hard distractor that is a real
alternative, and a goal that states an outcome without naming the answer."""
from __future__ import annotations

from cua_bench_s1.datagen.desktop_nav import DESKTOP_APPS, generate_dataset, generate_task
from cua_bench_s1.eval.adapter import OracleAdapter
from cua_bench_s1.eval.runner import run
from cua_bench_s1.eval.scoring import accuracy
from cua_bench_s1.task import ACTIONS, FAMILIES, CuaTask


def test_every_app_produces_a_schema_conformant_task():
    for app in DESKTOP_APPS:
        task = generate_task(app, seed=3)
        assert isinstance(task, CuaTask)
        assert task.family in FAMILIES
        assert task.modality_available == ["text"]
        assert task.ax_tree
        assert task.screenshot is None
        assert all(o.action in ACTIONS for o in task.options)
        assert set(task.expected) == {o.element_id for o in task.options}


def test_exactly_one_element_is_a_gold_click():
    for app in DESKTOP_APPS:
        task = generate_task(app, seed=11)
        clicks = [eid for eid, action in task.expected.items() if action == "click"]
        assert len(clicks) == 1


def test_hard_distractor_gets_a_real_click_option_with_gold_skip():
    for app in DESKTOP_APPS:
        task = generate_task(app, seed=5)
        hard_label = task.provenance["hard_distractor_label"]
        hard = [o for o in task.options if o.label == hard_label and o.action == "click"]
        assert hard, f"{app.app_id}: hard distractor has no click option to reject"
        assert task.expected[hard[0].element_id] == "skip"


def test_goal_is_stated_as_an_effect_so_label_matching_is_not_enough():
    # The request is generated from the command's EFFECT, never from its
    # label. Some labels do share wording with their own effect, which is
    # deliberate -- but most tasks must not be solvable by looking for the
    # goal's words in a control label, or the family measures string matching.
    tasks = [generate_task(app, seed=seed) for app in DESKTOP_APPS for seed in range(20)]
    no_overlap = [t for t in tasks
                  if t.provenance["target_label"].lower() not in (t.goal or "").lower()]
    assert all(t.goal for t in tasks)
    assert len(no_overlap) > 0.8 * len(tasks)


def test_generation_is_deterministic_in_its_seed():
    app = DESKTOP_APPS[0]
    first, second = generate_task(app, seed=7), generate_task(app, seed=7)
    assert first.ax_tree == second.ax_tree
    assert first.expected == second.expected
    assert first.goal == second.goal


def test_dataset_is_stable_across_calls_and_scores_perfectly_under_oracle():
    tasks = generate_dataset(n_per_app=2, seed=0)
    assert len(tasks) == len(DESKTOP_APPS) * 2
    # `stable_digest`, not the salted built-in hash: same arguments, same ids.
    assert [t.id for t in tasks] == [t.id for t in generate_dataset(n_per_app=2, seed=0)]
    assert accuracy(run(tasks, OracleAdapter(), "text")) == 1.0
