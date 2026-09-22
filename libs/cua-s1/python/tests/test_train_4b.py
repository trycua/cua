from __future__ import annotations

import sys
from pathlib import Path

TRAINING_ROOT = Path(__file__).resolve().parents[2] / "training"
sys.path.insert(0, str(TRAINING_ROOT))

try:
    from cua_bench_s1.task import CuaTask, OptionSpec  # noqa: E402
except ImportError:
    BENCH_S1_SRC = Path(__file__).resolve().parents[3] / "cua-bench-s1" / "python" / "src"
    sys.path.insert(0, str(BENCH_S1_SRC))
    from cua_bench_s1.task import CuaTask, OptionSpec  # noqa: E402

from train_4b import build_example, gold_option_letters  # noqa: E402

from cua_s1.four_b import assign_letters  # noqa: E402


def _task(**overrides) -> CuaTask:
    defaults = dict(
        id="t1",
        family="form_filling",
        app="TestApp",
        modality_available=["text"],
        screenshot=None,
        ax_tree="<tree/>",
        ax_tree_source="synthetic",
        elements=[],
        elements_source="synthetic_spec",
        entities=[{"id": "e1", "label": "Email", "value": "a@b.com"}],
        options=[
            OptionSpec(element_id="submit", role="Button", label="Submit", action="click"),
            OptionSpec(
                element_id="email", role="Edit", label="Email", action="fill", entity_id="e1"
            ),
            OptionSpec(element_id="email", role="Edit", label="Email", action="skip"),
        ],
        expected={"submit": "skip", "email": "fill"},
    )
    defaults.update(overrides)
    return CuaTask(**defaults)


class _FakeTokenizer:
    """Maps each option letter to a distinct single token id; anything else
    (i.e. the full chat-templated prompt text) to a fixed multi-token
    stand-in id sequence, matching real tokenizer behavior closely enough for
    this test's purposes (mirrors the fake used in test_four_b.py)."""

    def __init__(self):
        self._letter_ids = {"A": 10, "B": 11, "C": 12}

    def encode(self, text, add_special_tokens=False):
        if text in self._letter_ids:
            return [self._letter_ids[text]]
        return [1, 2, 3]

    def apply_chat_template(self, messages, tokenize=False, add_generation_prompt=True):
        return "<chat-text>"

    def __call__(self, text, return_tensors="pt"):
        import torch

        return _FakeBatchOutput(torch.tensor([[1, 2, 3, 4]]))


class _FakeBatchOutput:
    def __init__(self, input_ids):
        self.input_ids = input_ids


def test_gold_option_letters_puts_mass_on_every_matching_expected_action():
    task = _task()
    from train_4b import _task_options

    options = _task_options(task)
    assignment = assign_letters(options)

    letters = gold_option_letters(task, assignment)

    # "submit" is expected "skip" but its only option is "click" -> not gold.
    # "email" is expected "fill", and its "fill" option (index 1, letter B) is gold;
    # its "skip" option (index 2, letter C) is not.
    assert letters == ["B"]


def test_build_example_returns_none_when_no_option_matches_expected():
    task = _task(expected={"submit": "fill", "email": "click"})
    tokenizer = _FakeTokenizer()

    example = build_example(task, tokenizer, modality="text")

    assert example is None


def test_build_example_builds_soft_target_over_gold_letters():
    task = _task()
    tokenizer = _FakeTokenizer()

    example = build_example(task, tokenizer, modality="text")

    assert example is not None
    assert example["task_id"] == "t1"
    # 3 options -> letters A, B, C -> 3 letter token ids.
    assert example["letter_token_ids"] == [10, 11, 12]
    # Only option B ("email" -> "fill") is gold; all probability mass on it.
    assert example["target"] == [0.0, 1.0, 0.0]
    assert "mm_inputs" not in example


def test_build_example_multimodal_requires_a_processor():
    task = _task(
        modality_available=["multimodal"],
        screenshot="shot.png",
        ax_tree=None,
    )
    tokenizer = _FakeTokenizer()

    try:
        build_example(task, tokenizer, modality="multimodal", processor=None)
    except ValueError as e:
        assert "requires a processor" in str(e)
    else:
        raise AssertionError("expected ValueError")
