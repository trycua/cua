from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

torch = pytest.importorskip("torch")

TRAINING_DIR = Path(__file__).resolve().parents[2] / "training"


def _load_module(name: str):
    spec = importlib.util.spec_from_file_location(name, TRAINING_DIR / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def nano_data():
    pytest.importorskip("cua_bench_s1")
    return _load_module("nano_data")


@pytest.fixture(scope="module")
def train_nano(nano_data):
    return _load_module("train_nano")


def _make_task(cua_bench_s1, task_id: str, gold_action: str = "click"):
    OptionSpec = cua_bench_s1.OptionSpec
    CuaTask = cua_bench_s1.CuaTask
    options = [
        OptionSpec(element_id="submit", role="Button", label="Submit", action="click"),
        OptionSpec(element_id="submit", role="Button", label="Submit", action="skip"),
    ]
    return CuaTask(
        id=task_id,
        family="form_filling",
        app="demo",
        modality_available=["text"],
        screenshot=None,
        ax_tree="Button 'Submit' at (0,0,10,10)",
        ax_tree_source="synthetic",
        elements=[{"id": "submit", "role": "Button", "label": "Submit", "frame": [0, 0, 10, 10]}],
        elements_source="synthetic_spec",
        entities=[],
        options=options,
        expected={"submit": gold_action},
    )


@pytest.fixture(scope="module")
def cua_bench_s1_task_module():
    return pytest.importorskip("cua_bench_s1.task")


def test_build_parser_has_base_and_finetune_subcommands(train_nano):
    parser = train_nano.build_parser()
    args = parser.parse_args(
        ["base", "--data", "some/dir", "--modality", "text", "--width", "8", "--rank", "4"]
    )
    assert args.stage == "base"
    assert args.width == 8
    assert args.func is train_nano.cmd_base

    args = parser.parse_args(
        [
            "finetune",
            "--data",
            "some/dir",
            "--modality",
            "text",
            "--checkpoint-in",
            "in",
            "--checkpoint-out",
            "out",
        ]
    )
    assert args.stage == "finetune"
    assert args.func is train_nano.cmd_finetune


def test_explode_task_builds_one_labeled_example_per_element(nano_data, cua_bench_s1_task_module):
    task = _make_task(cua_bench_s1_task_module, "task-0", gold_action="click")
    examples = nano_data.explode_task(task, "text")
    assert len(examples) == 1
    example = examples[0]
    assert example.element_id == "submit"
    assert example.label == 0  # "click" is options[0]
    assert example.action == "click"


def test_tiny_base_training_run_produces_loadable_checkpoint(
    tmp_path, nano_data, train_nano, cua_bench_s1_task_module
):
    tasks = [
        _make_task(cua_bench_s1_task_module, f"task-{i}", gold_action=("click" if i % 2 == 0 else "skip"))
        for i in range(8)
    ]
    tasks_path = tmp_path / "tasks.jsonl"
    cua_bench_s1_task_module.save_jsonl(tasks, tasks_path)

    data_dir = tmp_path / "data"
    nano_data.build_split(tasks_path, "text", data_dir, "train")
    nano_data.build_split(tasks_path, "text", data_dir, "validation")

    checkpoint_dir = tmp_path / "checkpoint"
    model, _ = train_nano.make_nano_system(
        {"width": 8, "rank": 4, "option_tokens": 16, "context_tokens": 32}, "cpu"
    )
    collator = nano_data.NanoPreparedCollator("text", option_tokens=16, context_tokens=32)
    train_set = nano_data.NanoPreparedDataset(data_dir / "train.jsonl", "text")
    val_set = nano_data.NanoPreparedDataset(data_dir / "validation.jsonl", "text")

    summary = train_nano.run_training(
        model,
        train_set,
        val_set,
        collator,
        torch.device("cpu"),
        epochs=2,
        batch_size=4,
        learning_rate=1e-3,
        output=checkpoint_dir,
        extra_metadata={"stage": "base", "modality": "text"},
    )
    assert Path(summary["checkpoint"]).exists()

    loaded_model, loaded_collator, config = train_nano.load_nano_checkpoint(checkpoint_dir, "cpu")
    assert config["width"] == 8
    assert config["option_tokens"] == 16
    from cua_s1.nano import NanoElement

    elements = [nano_data and NanoElement("submit", "Button 'Submit'", ("click", "skip"))]
    scores = loaded_model.score_elements(elements, loaded_collator)
    assert "submit" in scores
    assert set(scores["submit"]) == {0, 1}
