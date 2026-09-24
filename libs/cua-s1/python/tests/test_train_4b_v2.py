from __future__ import annotations

import sys
from pathlib import Path

import pytest
import torch

TRAINING_ROOT = Path(__file__).resolve().parents[2] / "training"
sys.path.insert(0, str(TRAINING_ROOT))

try:
    from cua_bench_s1.task import CuaTask, OptionSpec  # noqa: E402
except ImportError:
    BENCH_S1_SRC = Path(__file__).resolve().parents[3] / "cua-bench-s1" / "python" / "src"
    sys.path.insert(0, str(BENCH_S1_SRC))
    from cua_bench_s1.task import CuaTask, OptionSpec  # noqa: E402

from cua_bench_s1.eval.adapter import option_key  # noqa: E402
from cua_bench_s1.eval.scoring import score_task  # noqa: E402
from train_4b_v2 import (  # noqa: E402
    _task_options,
    build_parser,
    check_val_split,
    element_groups,
    evaluate_val,
    example_loss,
    group_log_probs,
    lr_schedule,
    plan_example,
)

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
            # "submit" is uncontested: one option, so it cannot be got wrong.
            OptionSpec(element_id="submit", role="Button", label="Submit", action="click"),
            OptionSpec(
                element_id="email", role="Edit", label="Email", action="fill", entity_id="e1"
            ),
            OptionSpec(element_id="email", role="Edit", label="Email", action="skip"),
        ],
        expected={"submit": "click", "email": "fill"},
    )
    defaults.update(overrides)
    return CuaTask(**defaults)


class _FakeTokenizer:
    """Maps each option letter to a distinct single token id; anything else
    (i.e. the chat-templated prompt text) to a fixed stand-in id sequence."""

    def __init__(self):
        self._letter_ids = {letter: 10 + i for i, letter in enumerate("ABCDEFGH")}

    def encode(self, text, add_special_tokens=False):
        if text in self._letter_ids:
            return [self._letter_ids[text]]
        return [1, 2, 3]

    def apply_chat_template(self, messages, tokenize=False, add_generation_prompt=True):
        return "<chat-text>"

    def __call__(self, text, return_tensors="pt"):
        return _FakeBatchOutput(torch.tensor([[1, 2, 3, 4]]))


class _FakeBatchOutput:
    def __init__(self, input_ids):
        self.input_ids = input_ids


class _FakeOutput:
    def __init__(self, logits):
        self.logits = logits


class _FakeModel:
    """Returns a fixed vocabulary logit row at the final position, so the
    per-element readout can be checked against a known distribution."""

    device = torch.device("cpu")
    dtype = torch.float32

    def __init__(self, logits_by_token_id: dict[int, float]):
        vocab = torch.full((1, 4, 64), -10.0)
        for token_id, value in logits_by_token_id.items():
            vocab[0, -1, token_id] = value
        self._out = _FakeOutput(vocab)

    def __call__(self, **kwargs):
        return self._out

    def eval(self):
        return self

    def train(self):
        return self


def test_element_groups_drops_uncontested_elements():
    task = _task()
    assignment = assign_letters(_task_options(task))

    groups = element_groups(task, assignment)

    # Only "email" is contested; "submit"'s single option is zero-gradient and
    # the scorer marks it correct unconditionally.
    assert len(groups) == 1
    idxs, gold_positions = groups[0]
    assert [assignment.options[i].element_id for i in idxs] == ["email", "email"]
    assert gold_positions == [0]  # the "fill" option


def test_element_groups_puts_uniform_mass_over_several_gold_actions():
    task = _task(
        options=[
            OptionSpec("box", "CheckBox", "Agree", "check"),
            OptionSpec("box", "CheckBox", "Agree", "click"),
            OptionSpec("box", "CheckBox", "Agree", "skip"),
        ],
        expected={"box": "check"},
    )
    assignment = assign_letters(_task_options(task))
    ((idxs, gold_positions),) = element_groups(task, assignment)
    assert len(idxs) == 3
    assert gold_positions == [0]


def test_element_groups_skips_an_element_whose_gold_is_not_among_its_options():
    task = _task(expected={"submit": "click", "email": "scroll"})
    assignment = assign_letters(_task_options(task))
    assert element_groups(task, assignment) == []


def test_plan_example_returns_none_when_nothing_is_contested():
    task = _task(
        options=[OptionSpec("submit", "Button", "Submit", "click")],
        expected={"submit": "click"},
    )
    assert plan_example(task, _FakeTokenizer()) is None


def test_plan_example_carries_letter_ids_groups_and_a_tokenized_prompt():
    plan = plan_example(_task(), _FakeTokenizer())

    assert plan is not None
    assert plan["task_id"] == "t1"
    assert plan["letter_token_ids"] == [10, 11, 12]  # A, B, C
    assert len(plan["groups"]) == 1
    assert plan["input_ids"].tolist() == [1, 2, 3, 4]


def test_plan_example_multimodal_needs_the_screenshot_on_disk(tmp_path):
    task = _task(modality_available=["multimodal"], screenshot="shot.png", ax_tree=None)
    assert plan_example(task, _FakeTokenizer(), modality="multimodal", data_root=tmp_path) is None

    (tmp_path / "shot.png").write_bytes(b"")
    plan = plan_example(task, _FakeTokenizer(), modality="multimodal", data_root=tmp_path)
    assert plan is not None
    assert plan["image_path"] == tmp_path / "shot.png"
    assert "input_ids" not in plan  # built lazily, per step


def test_per_element_readout_matches_eval_scoring_on_the_same_logits():
    """The loss and the readout are scoped per element because that is what
    `eval.scoring` thresholds. Feeding one fixed logit row through both must
    give the same verdict."""
    from torch.nn import functional as F

    task = _task()
    plan = plan_example(task, _FakeTokenizer())
    # A: submit/click, B: email/fill, C: email/skip. "skip" wins its element,
    # so the task is WRONG even though the globally-largest logit is gold.
    model = _FakeModel({10: 5.0, 11: 0.0, 12: 1.0})

    log_probs = group_log_probs(
        model, plan, {"input_ids": plan["input_ids"].unsqueeze(0)}, torch, F
    )
    readout_correct = all(
        int(lp.argmax().item()) in set(gold)
        for lp, (_, gold) in zip(log_probs, plan["groups"], strict=False)
    )

    # The same distribution, expressed the way the scorer consumes it: each
    # element's own options renormalized among themselves.
    probs = {option_key("submit", "click"): 1.0}
    email = torch.softmax(torch.tensor([0.0, 1.0]), dim=-1).tolist()
    probs[option_key("email", "fill")] = email[0]
    probs[option_key("email", "skip")] = email[1]

    assert readout_correct is score_task(task, probs).correct is False


def test_example_loss_is_zero_when_every_contested_element_is_certain():
    from torch.nn import functional as F

    plan = plan_example(_task(), _FakeTokenizer())
    # Overwhelming mass on B (email/fill), the gold option for its element.
    model = _FakeModel({10: 0.0, 11: 60.0, 12: 0.0})

    loss = example_loss(model, plan, {"input_ids": plan["input_ids"].unsqueeze(0)}, torch, F)

    assert float(loss) == pytest.approx(0.0, abs=1e-6)


def test_evaluate_val_counts_a_task_only_when_every_element_is_right():
    from torch.nn import functional as F

    plan = plan_example(_task(), _FakeTokenizer())
    assert evaluate_val(_FakeModel({10: 0.0, 11: 5.0, 12: 0.0}), [plan], torch, F) == 1.0
    assert evaluate_val(_FakeModel({10: 0.0, 11: 0.0, 12: 5.0}), [plan], torch, F) == 0.0
    assert evaluate_val(_FakeModel({}), [], torch, F) == 0.0


def test_lr_schedule_warms_up_then_decays_to_min_lr():
    kwargs = dict(base_lr=1e-4, min_lr=1e-6, warmup=10, total_steps=100)

    assert lr_schedule(0, **kwargs) == pytest.approx(1e-5)
    assert lr_schedule(9, **kwargs) == pytest.approx(1e-4)  # end of warmup
    assert lr_schedule(99, **kwargs) == pytest.approx(
        1e-6, rel=0.05
    )  # approaching the cosine floor
    assert lr_schedule(100, **kwargs) == pytest.approx(1e-6)  # the floor itself
    # Monotonic decay after warmup.
    post = [lr_schedule(s, **kwargs) for s in range(10, 100)]
    assert all(a >= b for a, b in zip(post, post[1:], strict=False))


def test_check_val_split_refuses_a_test_split():
    check_val_split("runs/split/validation.jsonl")  # fine
    with pytest.raises(SystemExit):
        check_val_split("runs/split/test.jsonl")


def test_parser_defaults_and_modality_choices():
    args = build_parser().parse_args(["--train", "t.jsonl", "--val", "v.jsonl", "--out", "o"])

    assert args.modality == "text"
    assert args.batch_size == 8  # gradient accumulation, not per-example steps
    assert args.warmup_frac == 0.05
    assert args.augment is True

    mm = build_parser().parse_args(
        [
            "--train",
            "t.jsonl",
            "--val",
            "v.jsonl",
            "--out",
            "o",
            "--modality",
            "multimodal",
            "--no-augment",
        ]
    )
    assert (mm.modality, mm.augment) == ("multimodal", False)

    with pytest.raises(SystemExit):
        build_parser().parse_args(
            ["--train", "t.jsonl", "--val", "v.jsonl", "--out", "o", "--modality", "audio"]
        )
