"""Tests for the synthetic task generator: it must produce schema-conformant
CuaTasks that exercise the full schema+eval path without network access or
any real external dataset present."""
from __future__ import annotations

from cua_bench_s1.datagen.generator import generate_dataset, generate_task
from cua_bench_s1.datagen.specs import APPS_BY_FAMILY, EXAMPLE_APPS
from cua_bench_s1.eval.adapter import OracleAdapter
from cua_bench_s1.eval.runner import run
from cua_bench_s1.eval.scoring import accuracy
from cua_bench_s1.task import CuaTask, FAMILIES


def test_generate_task_produces_valid_task(tmp_path):
    app = EXAMPLE_APPS[0]
    task = generate_task(app, seed=1, modality_available=("text",), out_dir=tmp_path)
    assert isinstance(task, CuaTask)
    assert task.family in FAMILIES
    assert task.ax_tree is not None
    assert task.screenshot is None
    assert task.options
    assert set(task.expected.keys()) == {opt.element_id for opt in task.options}


def test_generate_task_multimodal_writes_screenshot(tmp_path):
    app = EXAMPLE_APPS[0]
    task = generate_task(app, seed=2, modality_available=("multimodal",), out_dir=tmp_path)
    assert task.screenshot is not None
    assert (tmp_path / task.screenshot).exists()


def test_generate_task_is_deterministic_given_seed(tmp_path):
    app = EXAMPLE_APPS[0]
    t1 = generate_task(app, seed=42, modality_available=("text",), out_dir=tmp_path)
    t2 = generate_task(app, seed=42, modality_available=("text",), out_dir=tmp_path)
    assert t1.ax_tree == t2.ax_tree
    assert t1.expected == t2.expected


def test_every_family_has_at_least_one_example_app():
    synthetic_families = {"form_filling", "login_auth", "consent_checkbox",
                          "multi_step_submit", "pagination", "search_filter",
                          "safety_gate"}
    assert synthetic_families.issubset(APPS_BY_FAMILY.keys())


def test_generated_dataset_scores_perfectly_under_oracle(tmp_path):
    apps = EXAMPLE_APPS[:3]
    tasks = generate_dataset(apps, n_per_app=2, seed=0, modality_available=("text",), out_dir=tmp_path)
    assert len(tasks) == len(apps) * 2
    results = run(tasks, OracleAdapter(), "text")
    assert accuracy(results) == 1.0
