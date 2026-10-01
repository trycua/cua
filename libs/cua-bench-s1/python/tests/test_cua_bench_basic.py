"""Tests for the `cua_bench_basic` family: the conversion logic, and -- when
the `cua_bench` SDK and its Playwright provider are actually installed -- two
real live task envs end to end.

The live tests use cua-bench's own `simulated` (Playwright) provider, the
same one cua-bench's own test suite uses, so nothing here needs Docker, QEMU
or a GPU. They still need the `cua_bench` package plus a downloaded Playwright
browser, which the `cua-bench-s1` project environment deliberately does not
depend on -- so they skip cleanly rather than failing when it is absent.
"""
from __future__ import annotations

import asyncio
import random
from pathlib import Path

import pytest

from cua_bench_s1.datagen.cua_bench_basic import (
    ENV_NAMES,
    FAMILY,
    ActionRecord,
    DomElement,
    RecordedEpisode,
    RecordedStep,
    assign_splits,
    build_candidates,
    convert_episode,
    dom_elements_from_query,
    render_ax_tree,
    role_for,
    target_element,
)
from cua_bench_s1.task import FAMILIES, dataset_hash


def _dataset_dir() -> Path | None:
    here = Path(__file__).resolve()
    for parent in here.parents:
        candidate = parent / "libs" / "cua-bench" / "datasets" / "cua-bench-basic"
        if candidate.is_dir():
            return candidate
    return None


DATASET_DIR = _dataset_dir()

try:  # pragma: no cover - environment-dependent
    import cua_bench  # noqa: F401
    import playwright  # noqa: F401

    _LIVE = DATASET_DIR is not None
except Exception:  # pragma: no cover
    _LIVE = False

live_only = pytest.mark.skipif(
    not _LIVE, reason="cua_bench + playwright + datasets/cua-bench-basic not available"
)


# --------------------------------------------------------------------------
# Fixtures: a small, realistic hand-built recorded episode (same shape the
# live recorder produces) so the conversion logic is testable without a
# browser.
# --------------------------------------------------------------------------


def _elements() -> list[DomElement]:
    return [
        DomElement(id="dom_0", role="Button", label="Submit", frame=[24, 124, 120, 166], tag="BUTTON"),
        DomElement(id="dom_1", role="Button", label="Cancel", frame=[136, 124, 232, 166], tag="BUTTON"),
        DomElement(id="dom_2", role="Edit", label="Username", frame=[24, 60, 232, 100], tag="INPUT"),
    ]


def _episode(task_index: int = 0, text: str = "Hello World") -> RecordedEpisode:
    els = _elements()
    return RecordedEpisode(
        env_name="typing-input",
        task_index=task_index,
        instruction=f'Type "{text}" into the Username field.',
        metadata={"text": text, "field_label": "Username"},
        steps=[
            RecordedStep(0, els, ActionRecord("ClickAction", {"x": 128, "y": 80}), None),
            RecordedStep(1, els, ActionRecord("TypeAction", {"text": text}), None),
        ],
        oracle_reward=1.0,
        provider="simulated",
    )


# --------------------------------------------------------------------------
# Pure conversion logic
# --------------------------------------------------------------------------


class TestPrimitives:
    def test_family_is_registered(self):
        assert FAMILY == "cua_bench_basic"
        assert FAMILY in FAMILIES

    def test_thirteen_envs_named(self):
        assert len(ENV_NAMES) == 13

    def test_role_mapping(self):
        assert role_for("BUTTON", None, None) == "Button"
        assert role_for("INPUT", "checkbox", None) == "CheckBox"
        assert role_for("INPUT", "range", None) == "Slider"
        assert role_for("SELECT", None, None) == "Select"
        assert role_for("DIV", None, "button") == "Button"

    def test_dom_query_translated_to_screen_space(self):
        raw = [{"tag": "BUTTON", "type": None, "dom_id": "submit", "role": None,
                "label": "Submit", "x": 24, "y": 76, "width": 96, "height": 42}]
        els = dom_elements_from_query(raw, offset_x=0, offset_y=48)
        assert els[0].frame == [24, 124, 120, 166]
        assert els[0].center == (72, 145)
        assert els[0].contains(72, 145)
        assert not els[0].contains(72, 90)

    def test_target_element_picks_smallest_containing_box(self):
        container = DomElement("dom_c", "Generic", "container", [0, 0, 300, 300], "DIV")
        button = DomElement("dom_b", "Button", "Submit", [24, 124, 120, 166], "BUTTON")
        hit = target_element(ActionRecord("ClickAction", {"x": 72, "y": 145}), [container, button])
        assert hit is button

    def test_action_describe_is_grounded_in_a_real_element(self):
        desc = ActionRecord("ClickAction", {"x": 72, "y": 145}).describe(_elements())
        assert 'Button "Submit"' in desc and "(72, 145)" in desc
        assert ActionRecord("TypeAction", {"text": "abc"}).describe([]) == "type 'abc'"


class TestAxTree:
    def test_renders_instruction_history_and_real_boxes(self):
        text = render_ax_tree(
            instruction="Click the Submit button.",
            step_index=2,
            prior_actions=["click at (200, 165) on Edit \"Username\"", "type 'abc'"],
            elements=_elements(),
        )
        assert text.splitlines()[0] == "Task: Click the Submit button."
        assert "Step: 2" in text
        assert "  1. click at (200, 165) on Edit \"Username\"" in text
        assert "  2. type 'abc'" in text
        assert '  - Button "Cancel" [136,124,232,166]' in text

    def test_leaks_no_gold_marker(self):
        text = render_ax_tree(instruction="i", step_index=0, prior_actions=[],
                              elements=_elements())
        assert "gold" not in text.lower() and "expected" not in text.lower()


class TestCandidates:
    def test_gold_present_exactly_once_and_capped(self):
        gold = ActionRecord("ClickAction", {"x": 72, "y": 145})
        cands = build_candidates(
            gold=gold, elements=_elements(), peer_actions=[],
            max_options=4, rng=random.Random(0),
        )
        assert len(cands) == 4
        assert sum(1 for c in cands if c.key() == gold.key()) == 1

    def test_peer_actions_are_preferred_distractors(self):
        gold = ActionRecord("TypeAction", {"text": "Hello World"})
        peer = ActionRecord("TypeAction", {"text": "Testing 123"})
        cands = build_candidates(
            gold=gold, elements=_elements(), peer_actions=[peer],
            max_options=3, rng=random.Random(1),
        )
        assert peer.key() in {c.key() for c in cands}

    def test_same_target_type_confusions_are_offered(self):
        gold = ActionRecord("ClickAction", {"x": 72, "y": 145})
        cands = build_candidates(
            gold=gold, elements=_elements(), peer_actions=[],
            max_options=10, rng=random.Random(2),
        )
        kinds = {c.kind for c in cands}
        assert "DoubleClickAction" in kinds and "RightClickAction" in kinds

    def test_every_distractor_is_grounded(self):
        """No fabricated noise: every non-gold candidate either reuses the
        gold's own real coordinates, or lands on a real element of the real
        page, or is a real peer-episode oracle action."""
        gold = ActionRecord("ClickAction", {"x": 72, "y": 145})
        els = _elements()
        cands = build_candidates(
            gold=gold, elements=els, peer_actions=[], max_options=10, rng=random.Random(3)
        )
        for c in cands:
            pt = c.point()
            assert pt is not None
            assert pt == (72.0, 145.0) or target_element(c, els) is not None

    def test_determinism(self):
        args = dict(gold=ActionRecord("ClickAction", {"x": 72, "y": 145}),
                    elements=_elements(), peer_actions=[], max_options=5)
        a = [c.key() for c in build_candidates(**args, rng=random.Random(7))]
        b = [c.key() for c in build_candidates(**args, rng=random.Random(7))]
        assert a == b


class TestConvertEpisode:
    def test_one_task_per_real_step(self):
        tasks = convert_episode(_episode(), split_name="train", modality_available=("text",))
        assert len(tasks) == 2
        assert [t.id for t in tasks] == [
            "cbb_typing-input_0_0",
            "cbb_typing-input_0_1",
        ]

    def test_exactly_one_gold_click_per_task(self):
        for t in convert_episode(_episode(), split_name="train", modality_available=("text",)):
            assert sorted(t.expected.values()).count("click") == 1
            assert set(t.expected) == {e["id"] for e in t.elements}

    def test_every_element_gets_a_real_click_skip_pair(self):
        t = convert_episode(_episode(), split_name="train", modality_available=("text",))[0]
        for el in t.elements:
            acts = {o.action for o in t.options if o.element_id == el["id"]}
            assert acts == {"click", "skip"}

    def test_ax_tree_carries_instruction_history_and_real_boxes(self):
        tasks = convert_episode(_episode(), split_name="train", modality_available=("text",))
        first, second = tasks[0].ax_tree, tasks[1].ax_tree
        assert "Type \"Hello World\"" in first
        assert "(none -- this is the first step)" in first
        # Step 1's state must include step 0's real action as history.
        assert "click at (128, 80)" in second
        assert 'Button "Submit" [24,124,120,166]' in first

    def test_history_dependence_makes_the_two_steps_distinguishable(self):
        tasks = convert_episode(_episode(), split_name="train", modality_available=("text",))
        assert tasks[0].ax_tree != tasks[1].ax_tree

    def test_peer_episode_supplies_a_real_hard_distractor(self):
        ep, peer = _episode(0, "Hello World"), _episode(1, "Testing 123")
        tasks = convert_episode(ep, split_name="train", peer_episodes=[ep, peer],
                                modality_available=("text",))
        type_labels = {o.label for o in tasks[1].options}
        assert "type 'Hello World'" in type_labels
        assert "type 'Testing 123'" in type_labels

    def test_provenance_discloses_gold_source_and_oracle_reward(self):
        p = convert_episode(_episode(), split_name="val", modality_available=("text",))[0].provenance
        assert p["gold_label_method"].startswith("cua_bench_reference_solution")
        assert p["oracle_reward"] == 1.0 and p["oracle_verified"] is True
        assert p["dataset_split"] == "val"
        assert p["env_name"] == "typing-input"
        assert "live_dom_query" in p["ax_tree_method"]

    def test_require_oracle_success_drops_unverified_episodes(self):
        ep = _episode()
        ep.oracle_reward = 0.0
        assert convert_episode(ep, split_name="train", modality_available=("text",),
                               require_oracle_success=True) == []
        # The default keeps the episode, but discloses the unverified oracle.
        kept = convert_episode(ep, split_name="train", modality_available=("text",))
        assert kept and kept[0].provenance["oracle_verified"] is False

    def test_multimodal_is_dropped_when_no_screenshot_was_captured(self):
        t = convert_episode(_episode(), split_name="train")[0]
        assert t.modality_available == ["text"]
        assert t.screenshot is None

    def test_tasks_are_hashable_as_a_dataset(self):
        tasks = convert_episode(_episode(), split_name="train", modality_available=("text",))
        assert len(dataset_hash(tasks)) == 64


class TestViewportVisibility:
    """The bundled pages really do overflow their windows under the
    `simulated` provider, so off-screen elements must never be offered and
    off-screen gold steps must never be emitted."""

    def _clipped(self) -> RecordedEpisode:
        ep = _episode()
        # A real window content rect that cuts the page off at y=198.
        ep.viewport = [0, 48, 256, 198]
        ep.steps[0].elements = ep.steps[0].elements + [
            DomElement("dom_9", "Button", "Below the fold", [24, 300, 120, 340], "BUTTON")
        ]
        return ep

    def test_offscreen_elements_are_not_offered_as_options(self):
        ep = self._clipped()
        t = convert_episode(ep, split_name="train", modality_available=("text",))[0]
        assert "Below the fold" not in t.ax_tree
        assert all("Below the fold" not in o.label for o in t.options)

    def test_step_with_offscreen_gold_is_dropped_but_stays_in_history(self):
        ep = self._clipped()
        ep.steps[0].gold_action = ActionRecord("ClickAction", {"x": 72, "y": 320})
        tasks = convert_episode(ep, split_name="train", modality_available=("text",))
        assert [t.id for t in tasks] == ["cbb_typing-input_0_1"]
        assert "click at (72, 320)" in tasks[0].ax_tree  # still real history

    def test_offscreen_gold_can_be_kept_explicitly(self):
        ep = self._clipped()
        ep.steps[0].gold_action = ActionRecord("ClickAction", {"x": 72, "y": 320})
        tasks = convert_episode(ep, split_name="train", modality_available=("text",),
                                drop_offscreen_gold=False)
        assert len(tasks) == 2

    def test_no_viewport_means_no_filtering(self):
        ep = self._clipped()
        ep.viewport = None
        t = convert_episode(ep, split_name="train", modality_available=("text",))[0]
        assert "Below the fold" in t.ax_tree

    def test_provenance_reports_exclusions_and_layout_fit(self):
        ep = self._clipped()
        ep.window_layout_fitted = True
        p = convert_episode(ep, split_name="train", modality_available=("text",))[0].provenance
        assert p["n_offscreen_elements_excluded"] == 1
        assert p["viewport"] == [0, 48, 256, 198]
        assert p["window_layout_fitted"] is True

    def test_peer_action_landing_offscreen_is_not_offered(self):
        ep, peer = self._clipped(), _episode(1, "Testing 123")
        peer.steps[0].gold_action = ActionRecord("ClickAction", {"x": 72, "y": 320})
        t = convert_episode(ep, split_name="train", peer_episodes=[ep, peer],
                            modality_available=("text",))[0]
        assert all("(72, 320)" not in o.label for o in t.options)


class TestSplits:
    def test_majority_in_train_and_val(self):
        for n in (5, 7):
            s = assign_splits(n)
            assert len(s["train"]) + len(s["val"]) > len(s["test"])

    def test_test_holds_out_the_last_fresh_parameterizations(self):
        s = assign_splits(7)
        assert s == {"train": [0, 1, 2, 3], "val": [4], "test": [5, 6]}

    def test_no_overlap_and_full_coverage(self):
        for n in range(1, 10):
            s = assign_splits(n)
            allidx = s["train"] + s["val"] + s["test"]
            assert sorted(allidx) == list(range(n))
            assert len(set(allidx)) == n

    def test_tiny_parameter_counts_degrade_honestly(self):
        assert assign_splits(1) == {"train": [0], "val": [], "test": []}
        assert assign_splits(2) == {"train": [0], "val": [], "test": [1]}
        assert assign_splits(3) == {"train": [0], "val": [1], "test": [2]}


# --------------------------------------------------------------------------
# Live end-to-end against two real cua-bench envs (skipped without cua_bench)
# --------------------------------------------------------------------------


@live_only
@pytest.mark.parametrize("env_name", ["click-button", "typing-input"])
def test_live_record_episode_produces_real_steps(env_name, tmp_path):
    from cua_bench_s1.datagen.cua_bench_basic import record_episode

    ep = asyncio.run(
        record_episode(env_dir=DATASET_DIR / env_name, task_index=0, out_dir=tmp_path)
    )
    assert ep.env_name == env_name
    assert ep.steps, "the reference solution took no actions"
    assert ep.oracle_reward == 1.0 and ep.oracle_verified
    for step in ep.steps:
        assert step.elements, "no real interactive elements found in the live DOM"
        assert Path(step.screenshot).exists()
    # Real gold coordinates must land on a real element of the real page.
    first = ep.steps[0]
    assert target_element(first.gold_action, first.elements) is not None


@live_only
def test_live_window_layout_fit_gives_the_page_its_requested_size(tmp_path):
    """Without the fit, the `simulated` provider clips these pages to ~150px
    and the real gold targets fall off-screen; with it, the window is the size
    the env's own `launch_window(...)` asked for."""
    from cua_bench_s1.datagen.cua_bench_basic import record_episode

    unfitted = asyncio.run(
        record_episode(env_dir=DATASET_DIR / "click-button", task_index=0,
                       out_dir=tmp_path, fit_layout=False, capture_screenshots=False)
    )
    fitted = asyncio.run(
        record_episode(env_dir=DATASET_DIR / "click-button", task_index=0,
                       out_dir=tmp_path, capture_screenshots=False)
    )
    assert unfitted.window_layout_fitted is False
    assert fitted.window_layout_fitted is True
    # click-button launches a 256x256 window.
    assert fitted.viewport[3] - fitted.viewport[1] == 256
    assert unfitted.viewport[3] - unfitted.viewport[1] < 200


@live_only
def test_live_conversion_end_to_end(tmp_path):
    from cua_bench_s1.datagen.cua_bench_basic import record_episode

    ep = asyncio.run(
        record_episode(env_dir=DATASET_DIR / "click-button", task_index=0, out_dir=tmp_path)
    )
    tasks = convert_episode(ep, split_name="train")
    assert tasks
    t = tasks[0]
    assert t.family == FAMILY and t.app == "click-button"
    assert "multimodal" in t.modality_available and "text" in t.modality_available
    assert sorted(t.expected.values()).count("click") == 1
    assert len(t.elements) >= 3, "too few real distractors extracted from the live page"


@live_only
def test_live_agentic_env_rollout_and_truncation():
    from cua_bench_s1.agentic import MAX_STEPS, CuaBenchBasicEnv

    async def run():
        async with CuaBenchBasicEnv(
            "click-button", dataset_dir=DATASET_DIR, task_index=0, max_steps=3
        ) as env:
            obs = await env.reset()
            assert obs.screenshot and obs.instruction
            assert obs.reward is None  # no fabricated per-step shaping reward
            from cua_bench.types import ClickAction

            # Deliberately useless clicks: the episode must truncate cleanly
            # at max_steps with a real failure signal, not raise.
            for _ in range(3):
                obs = await env.step(ClickAction(x=2, y=2))
            assert obs.done and obs.truncated
            assert obs.success is False
            assert env.step_count == 3
        assert MAX_STEPS == 20

    asyncio.run(run())


@live_only
def test_live_agentic_env_oracle_succeeds():
    from cua_bench_s1.agentic import CuaBenchBasicEnv

    async def run():
        async with CuaBenchBasicEnv("click-button", dataset_dir=DATASET_DIR) as env:
            await env.reset()
            assert await env.solve() == 1.0

    asyncio.run(run())


@live_only
def test_live_rollout_helper_scores_a_real_failing_policy():
    from cua_bench_s1.agentic import rollout

    async def policy(obs):
        # Deliberately off-target, and passed as a string to exercise
        # cua-bench's own action-string parsing.
        return "click(2, 2)"

    result = asyncio.run(
        rollout("click-button", policy=policy, dataset_dir=DATASET_DIR,
                task_index=0, max_steps=2)
    )
    assert result.env_name == "click-button"
    assert result.n_steps == 2 and result.truncated
    assert result.success is False and result.error is None


@live_only
def test_live_element_grounded_policy_really_solves_the_task():
    """An end-to-end RL-shaped check: a policy that reads the live element
    list, picks the element the real instruction names, and clicks its real
    center earns the environment's own real reward through `step()` -- not
    via `solve()`."""
    from cua_bench_s1.agentic import CuaBenchBasicEnv

    async def run():
        async with CuaBenchBasicEnv(
            "color-picker", dataset_dir=DATASET_DIR, task_index=0, max_steps=5
        ) as env:
            obs = await env.reset()
            els = await env.elements()
            assert els and all("frame" in e and "label" in e for e in els)

            target = obs.metadata["name"]  # e.g. "red"
            match = next(e for e in els if target.lower() in e["label"].lower())
            x0, y0, x1, y1 = match["frame"]

            from cua_bench.types import ClickAction, DoneAction

            await env.step(ClickAction(x=(x0 + x1) // 2, y=(y0 + y1) // 2))
            final = await env.step(DoneAction())
            assert final.done and not final.truncated
            assert final.success is True and final.reward == 1.0

    asyncio.run(run())


@live_only
def test_live_rollout_reports_a_policy_error_without_crashing():
    from cua_bench_s1.agentic import rollout

    async def policy(obs):
        raise ValueError("policy blew up")

    result = asyncio.run(
        rollout("click-button", policy=policy, dataset_dir=DATASET_DIR, task_index=0)
    )
    assert result.success is False
    assert result.error is not None and "policy blew up" in result.error
