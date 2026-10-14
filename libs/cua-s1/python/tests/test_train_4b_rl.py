from __future__ import annotations

import math
import sys
import types
from dataclasses import dataclass
from pathlib import Path

import pytest

TRAINING_ROOT = Path(__file__).resolve().parents[2] / "training"
sys.path.insert(0, str(TRAINING_ROOT))

try:
    from cua_bench_s1.task import CuaTask  # noqa: E402,F401
except ImportError:
    BENCH_S1_SRC = Path(__file__).resolve().parents[3] / "cua-bench-s1" / "python" / "src"
    sys.path.insert(0, str(BENCH_S1_SRC))
    from cua_bench_s1.task import CuaTask  # noqa: E402,F401

from train_4b_rl import (  # noqa: E402
    MAX_OPTIONS,
    brier_step_coefficient,
    build_options,
    build_options_multimodal,
    build_parser,
    build_task,
    build_task_multimodal,
    option_to_actions,
    prune_elements,
    quoted_values,
    rloo_advantage,
    state_suffix,
    task_options,
)

from cua_s1.four_b import assign_letters  # noqa: E402


@dataclass
class _Obs:
    """The fields of `cua_bench_s1.agentic.StepResult` this bridge reads."""

    instruction: str
    metadata: dict


def _element(i: int, label: str, tag: str = "BUTTON") -> dict:
    return {
        "id": f"el{i}",
        "role": "Button",
        "label": label,
        "tag": tag,
        "dom_id": f"dom{i}",
        "frame": [10 * i, 20, 10 * i + 8, 28],
    }


# --- live step -> bounded decision -----------------------------------------


def test_quoted_values_takes_quoted_spans_and_metadata_values():
    values = quoted_values('Enter "Hello World" into the box.', {"value": "Product"})
    assert values == ["Hello World", "Product"]
    assert quoted_values("", None) == []


def test_prune_elements_ignores_quoted_spans_when_ranking_targets():
    # The quoted span is the VALUE to type, not the target to act on: the
    # formula mentions A1, but the task is about cell C3.
    elements = [_element(i, f"Cell A{i}", tag="INPUT") for i in range(1, 12)]
    elements.append(_element(99, "Cell C3", tag="INPUT"))

    kept = prune_elements(elements, 'Enter "=SUM(A1:A10)" into cell C3.', limit=3)

    assert kept[0]["label"] == "Cell C3"  # ranked first, not left to page order


def test_prune_elements_is_a_no_op_below_the_limit():
    elements = [_element(i, f"Button {i}") for i in range(3)]
    assert prune_elements(elements, "click something", limit=11) == elements


def test_build_options_reserves_the_done_slot_and_admits_whole_elements():
    elements = [_element(i, f"Field {i}", tag="INPUT") for i in range(20)]

    options = build_options(elements, 'type "abc"', {})

    assert len(options) <= MAX_OPTIONS
    assert options[-1].action == "done"
    # No element is half-admitted: every non-terminal element offering `click`
    # also offers its `fill` and `skip`.
    per_element: dict[str, set[str]] = {}
    for option in options[:-1]:
        per_element.setdefault(option.element_id, set()).add(option.action)
    assert all(actions == {"fill", "click", "skip"} for actions in per_element.values())


def test_build_options_withholds_a_fill_the_field_already_holds():
    element = _element(1, "Name", tag="INPUT")
    instruction = 'Type "Product" into the field.'

    unknown_state = build_options([element], instruction, {})
    already_typed = build_options(
        [element], instruction, {}, values_state={"dom1": {"value": "Product"}}
    )

    assert any(o.action == "fill" for o in unknown_state)
    # Typing appends here, so re-offering it could only destroy the reward.
    assert not any(o.action == "fill" for o in already_typed)


def test_state_suffix_reports_live_values_and_toggle_state():
    assert state_suffix(_element(1, "Name"), {"dom1": {"value": "abc"}}) == ' value="abc"'
    assert state_suffix(_element(1, "Sw"), {"dom1": {"checked": True}}) == " checked=true"
    assert state_suffix(_element(1, "Name"), {}) == ""


def test_build_task_is_scorable_and_describes_only_admitted_elements():
    elements = [_element(i, f"Field {i}", tag="INPUT") for i in range(20)]
    obs = _Obs('Type "abc" somewhere.', {})

    task = build_task(obs, elements, "typing-input-0", 3)

    assert task.id == "typing-input-0-s3"
    assert task.goal == 'Type "abc" somewhere.'
    assert task.modality_available == ["text"]
    described = {line.split('"')[1] for line in task.ax_tree.splitlines()}
    admitted = {o.label for o in task.options if o.element_id != "__episode__"}
    assert described == admitted
    # The option set must fit the single-token letter budget the readout uses.
    assert len(assign_letters(task_options(task)).letters) == len(task.options)


def test_multimodal_options_name_marks_only_and_hide_the_page_text():
    marks = {1: _element(1, "Submit"), 2: _element(2, "Cancel")}

    options = build_options_multimodal(marks, "press the confirm control", {})

    labels = {o.label for o in options}
    assert "Submit" not in labels and "Cancel" not in labels
    assert {"mark 1", "mark 2"} <= labels


def test_build_task_multimodal_offers_no_text_fallback():
    marks = {1: _element(1, "Submit")}
    task = build_task_multimodal(_Obs("press it", {}), marks, "shot.png", "click-button-0", 0)

    assert task.modality_available == ["multimodal"]
    assert task.ax_tree is None
    assert task.screenshot == "shot.png"


# --- chosen option -> real environment actions -----------------------------


@pytest.fixture
def fake_cua_bench(monkeypatch):
    """`cua_bench` is the live environment package; stub its action types so
    the option -> action mapping can be tested without it installed."""

    @dataclass
    class ClickAction:
        x: int
        y: int

    @dataclass
    class TypeAction:
        text: str

    @dataclass
    class DoneAction:
        pass

    module = types.ModuleType("cua_bench.types")
    module.ClickAction, module.TypeAction, module.DoneAction = ClickAction, TypeAction, DoneAction
    parent = types.ModuleType("cua_bench")
    parent.types = module
    monkeypatch.setitem(sys.modules, "cua_bench", parent)
    monkeypatch.setitem(sys.modules, "cua_bench.types", module)
    return module


def test_option_to_actions_maps_click_fill_and_done(fake_cua_bench):
    elements = [_element(1, "Name", tag="INPUT")]
    options = build_options(elements, 'Type "abc" here.', {})
    by_action = {o.action: o for o in options}

    click = option_to_actions(by_action["click"], elements, ["abc"])
    assert [type(a).__name__ for a in click.actions] == ["ClickAction"]
    assert (click.actions[0].x, click.actions[0].y) == (14, 24)  # element centre

    # A fill is a focus click then a type: two real environment steps.
    fill = option_to_actions(by_action["fill"], elements, ["abc"])
    assert [type(a).__name__ for a in fill.actions] == ["ClickAction", "TypeAction"]
    assert fill.actions[1].text == "abc"

    done = option_to_actions(options[-1], elements, [])
    assert done.done is True


def test_option_to_actions_is_a_no_op_for_skip_and_unlocatable_elements(fake_cua_bench):
    elements = [_element(1, "Name")]
    skip = next(o for o in build_options(elements, "", {}) if o.action == "skip")
    assert option_to_actions(skip, elements, []).actions == []

    click = next(o for o in build_options(elements, "", {}) if o.action == "click")
    assert option_to_actions(click, [{"id": "el1", "frame": None}], []).actions == []


# --- the objective ----------------------------------------------------------


def test_rloo_advantage_is_reward_minus_the_other_episodes_mean():
    rewards = [1.0, 0.0, 0.0, 0.0]
    assert rloo_advantage(rewards, 0) == pytest.approx(1.0)
    assert rloo_advantage(rewards, 1) == pytest.approx(-1 / 3)
    # Zero-mean across the group, which is what makes the baseline unbiased.
    assert sum(rloo_advantage(rewards, k) for k in range(4)) == pytest.approx(0.0)


def test_rloo_advantage_vanishes_when_every_episode_agrees():
    assert all(rloo_advantage([1.0] * 4, k) == 0.0 for k in range(4))
    assert all(rloo_advantage([0.0] * 4, k) == 0.0 for k in range(4))


def test_rloo_advantage_falls_back_to_the_raw_reward_at_group_size_one():
    assert rloo_advantage([1.0], 0) == 1.0


def test_brier_step_coefficient_matches_the_analytic_derivative():
    # conf = exp(mean_t log pi_t); d (conf - r)^2 / d log pi_t = 2(conf-r)conf/T.
    step_log_probs = [-0.2, -0.5, -0.1]
    n_steps, reward = len(step_log_probs), 1.0
    confidence = math.exp(sum(step_log_probs) / n_steps)

    coefficient = brier_step_coefficient(confidence, reward, n_steps)

    eps = 1e-6
    bumped = list(step_log_probs)
    bumped[0] += eps
    numeric = ((math.exp(sum(bumped) / n_steps) - reward) ** 2 - (confidence - reward) ** 2) / eps
    assert coefficient == pytest.approx(numeric, rel=1e-4)


def test_brier_pushes_confidence_toward_the_realized_outcome():
    # The step coefficient multiplies log pi_t in a loss that is MINIMIZED, so
    # an overconfident failure must get a positive coefficient (push the taken
    # action's log-prob down) and an underconfident success a negative one.
    assert brier_step_coefficient(0.9, 0.0, 4) > 0
    assert brier_step_coefficient(0.2, 1.0, 4) < 0
    assert brier_step_coefficient(1.0, 1.0, 4) == pytest.approx(0.0)


# --- CLI --------------------------------------------------------------------


def test_parser_defaults_and_modality_choices():
    args = build_parser().parse_args(["--sft-adapter", "a", "--out", "o"])

    assert args.modality == "text"
    assert args.samples == 4  # the RLOO group size
    assert args.provider == "simulated"
    assert args.kl_weight == 0.05
    assert args.max_steps == 20

    mm = build_parser().parse_args(
        ["--sft-adapter", "a", "--out", "o", "--modality", "multimodal", "--envs", "toggle-switch"]
    )
    assert mm.modality == "multimodal"
    assert mm.envs == ["toggle-switch"]

    with pytest.raises(SystemExit):
        build_parser().parse_args(["--out", "o"])  # --sft-adapter is required
