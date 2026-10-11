"""Tests for the scoring harness: given a fake adapter with known outputs,
scoring must produce the expected accuracy/ECE/validity."""
from __future__ import annotations

from cua_bench_s1.eval.adapter import OracleAdapter, RandomAdapter, option_key
from cua_bench_s1.eval.runner import run, summarize
from cua_bench_s1.eval.scoring import (accuracy, element_accuracy,
                                       expected_calibration_error, score_task,
                                       validate_distribution)
from cua_bench_s1.task import CuaTask, OptionSpec


def _make_task(task_id: str, gold: str) -> CuaTask:
    return CuaTask(
        id=task_id,
        family="form_filling",
        app="demo_app",
        modality_available=["text"],
        screenshot=None,
        ax_tree="# Demo",
        ax_tree_source="synthetic",
        elements=[{"id": "el_0", "role": "Button", "label": "Submit", "frame": None}],
        elements_source="synthetic_spec",
        entities=[],
        options=[
            OptionSpec("el_0", "Button", "Submit", "click"),
            OptionSpec("el_0", "Button", "Submit", "skip"),
        ],
        expected={"el_0": gold},
    )


def test_oracle_adapter_scores_perfectly():
    tasks = [_make_task("t1", "click"), _make_task("t2", "skip")]
    results = run(tasks, OracleAdapter(), "text")
    assert accuracy(results) == 1.0
    assert element_accuracy(results) == 1.0
    assert expected_calibration_error(results) == 0.0
    for r in results:
        assert r.valid
        assert r.correct
        assert r.confidence == 1.0


def test_fake_adapter_with_known_wrong_output_scores_zero():
    class AlwaysClickAdapter:
        name = "always_click"

        def predict(self, task, modality):
            return {option_key("el_0", "click"): 1.0, option_key("el_0", "skip"): 0.0}

    tasks = [_make_task("t1", "skip")]
    results = run(tasks, AlwaysClickAdapter(), "text")
    assert accuracy(results) == 0.0
    assert results[0].valid
    assert results[0].per_element_predicted["el_0"] == "click"
    assert results[0].per_element_gold["el_0"] == "skip"


def test_invalid_distribution_is_fail_closed():
    task = _make_task("t1", "click")
    # Missing an option key entirely -> invalid, not an error.
    bad_probs = {option_key("el_0", "click"): 1.0}
    valid, reason = validate_distribution(task, bad_probs)
    assert not valid
    assert reason is not None
    result = score_task(task, bad_probs)
    assert not result.valid
    assert not result.correct
    assert result.confidence == 0.0


def test_summarize_reports_dataset_hash_and_accuracy():
    tasks = [_make_task("t1", "click"), _make_task("t2", "skip")]
    results = run(tasks, OracleAdapter(), "text")
    summary = summarize(results, tasks, "oracle", "text")
    assert summary["accuracy"] == 1.0
    assert summary["n_tasks"] == 2
    assert summary["n_invalid"] == 0
    assert "dataset_hash" in summary


def test_random_adapter_produces_valid_distributions():
    tasks = [_make_task("t1", "click")]
    results = run(tasks, RandomAdapter(seed=0), "text")
    assert results[0].valid
