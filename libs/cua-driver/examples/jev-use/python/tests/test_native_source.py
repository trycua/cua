"""NativeAccessibilitySource, stable IDs, policy, and AppKit tasks (RFC #4268)."""

from __future__ import annotations

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from choose_action import validate_request
from jev_adapter import choose_mock_for_task
from native import NativeObservation, eligible_controls, risk_categories, slug
from native_tasks import (
    APPKIT_TASK_IDS,
    MAX_EXECUTABLE_CANDIDATES,
    AppStateOracle,
    OracleError,
    appkit_task,
    compose,
    native_choice_request,
    visual_fallback_reason,
)
from sources import Candidate, NativeAccessibilitySource
from tasks import TaskSources

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "fixtures/native"


def window_state(name: str = "initial") -> dict:
    return json.loads((FIXTURES / f"appkit-window-state-{name}-v1.json").read_text(encoding="utf-8"))


def observe(payload: dict) -> NativeObservation:
    return NativeObservation.from_window_state(
        payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
    )


def element(index: int, role: str, label: str | None, **extra) -> dict:
    item = {
        "element_index": index,
        "role": role,
        "depth": 1,
        "parent_index": 0,
        "enabled": True,
        "element_token": f"s0000000a:{index}",
        "frame": {"x": 10 + index, "y": 10, "w": 40, "h": 20},
    }
    if label is not None:
        item["label"] = label
    item.update(extra)
    return item


def synthetic(elements: list[dict], **extra) -> dict:
    payload = {
        "pid": 7,
        "window_id": 9,
        "snapshot_id": "s0000000a",
        "capture_id": "cap-1",
        "elements_complete": False,
        "window_bounds": {"x": 0, "y": 0, "width": 500, "height": 400},
        "elements": [
            {"element_index": 0, "role": "AXWindow", "depth": 0, "label": "Main",
             "element_token": "s0000000a:0", "frame": {"x": 0, "y": 0, "w": 500, "h": 400}},
            *elements,
        ],
    }
    payload.update(extra)
    return payload


class CrossLanguageParityTest(unittest.TestCase):
    def test_shared_id_fixture(self) -> None:
        fixture = json.loads((FIXTURES / "native-candidate-ids-v1.json").read_text(encoding="utf-8"))
        for case in fixture["cases"]:
            ids = [c.id for c in eligible_controls(observe(case["window_state"]), case["platform"]).controls]
            self.assertEqual(ids, case["expected_ids"], case["name"])
        for label, expected in fixture["slugs"].items():
            self.assertEqual(slug(label), expected)
        for label, expected in fixture["risks"].items():
            self.assertEqual(sorted(risk_categories(label)), expected)


class ElementRulesTest(unittest.TestCase):
    def test_fixture_controls_and_exclusions(self) -> None:
        controls = eligible_controls(observe(window_state()), "macos")
        ids = [control.id for control in controls.controls]
        self.assertEqual(
            ids,
            [
                "ax:button:increment",
                "ax:button:reset",
                "ax:button:click-target-left-right-double",
                "ax:checkbox:i-agree",
                "ax:button:right-click-for-context-menu",
                "ax:button:exit",
                "ax:text_input:note",
                "ax:button:save-note",
                "ax:radio:small",
                "ax:radio:medium",
                "ax:radio:large",
            ],
        )
        # The placeholder-only field reports its placeholder as label and value.
        self.assertNotIn("ax:text_input:type-here", ids)
        # Menu-bar items lie outside the window; static text and sliders have no class.
        self.assertGreater(controls.excluded["off_screen"], 0)
        self.assertGreater(controls.excluded["unknown_role"], 0)
        self.assertGreater(controls.excluded["unlabeled"], 0)

    def test_each_rule_excludes(self) -> None:
        cases = {
            "unknown_role": element(1, "AXStaticText", "Hello"),
            "disabled": element(1, "AXButton", "Go", enabled=False),
            "off_screen": element(1, "AXButton", "Go", frame={"x": 900, "y": 900, "w": 10, "h": 10}),
            "zero size": element(1, "AXButton", "Go", frame={"x": 1, "y": 1, "w": 0, "h": 10}),
            "no frame": {k: v for k, v in element(1, "AXButton", "Go").items() if k != "frame"},
            "unlabeled": element(1, "AXButton", None),
            "blank label": element(1, "AXButton", "   "),
            "label equals value": element(1, "AXTextField", "typed", value="typed"),
            "marked unlabelled": element(1, "AXButton", "Go", unlabelled=True),
            "web content": element(1, "AXButton", "Go", in_web_content=True),
            "no token": {k: v for k, v in element(1, "AXButton", "Go").items() if k != "element_token"},
        }
        for name, item in cases.items():
            with self.subTest(name=name):
                self.assertEqual(eligible_controls(observe(synthetic([item])), "macos").controls, ())
        kept = eligible_controls(observe(synthetic([element(1, "AXButton", "Go")])), "macos")
        self.assertEqual([c.id for c in kept.controls], ["ax:button:go"])

    def test_platform_tables_apply(self) -> None:
        items = [element(1, "Edit", "Name"), element(2, "Text", "Static"), element(3, "push button", "OK")]
        windows = eligible_controls(observe(synthetic(items)), "windows")
        linux = eligible_controls(observe(synthetic(items)), "linux")
        # UIA Text is static text; "push button" normalizes to "button" as Driver does.
        self.assertEqual([c.id for c in windows.controls], ["ax:text_input:name", "ax:button:ok"])
        self.assertEqual(
            [c.id for c in linux.controls],
            ["ax:text_input:static", "ax:button:ok"],  # "Edit" is not an AT-SPI role
        )

    def test_secret_labels_are_redacted(self) -> None:
        item = element(1, "AXButton", "Send hunter2 now")
        controls = eligible_controls(
            observe(synthetic([item])), "macos", redact=lambda value: value.replace("hunter2", "[secret]")
        )
        self.assertEqual(controls.controls[0].label, "Send [secret] now")
        self.assertNotIn("hunter2", controls.controls[0].id)


class StableIdTest(unittest.TestCase):
    def test_ids_ignore_element_index(self) -> None:
        first = synthetic([element(1, "AXButton", "Apply"), element(2, "AXCheckBox", "Wrap")])
        shifted = copy.deepcopy(first)
        for item in shifted["elements"][1:]:
            item["element_index"] += 40
            item["element_token"] = f"s0000000b:{item['element_index']}"
        a = [c.id for c in eligible_controls(observe(first), "macos").controls]
        b = [c.id for c in eligible_controls(observe(shifted), "macos").controls]
        self.assertEqual(a, b)
        # Across real snapshots too.
        initial = [c.id for c in eligible_controls(observe(window_state()), "macos").controls]
        after = [c.id for c in eligible_controls(observe(window_state("after-actions")), "macos").controls]
        self.assertEqual(initial, after)

    def test_collisions_get_path_suffixes(self) -> None:
        payload = synthetic(
            [
                element(1, "AXButton", "Options"),
                element(2, "AXButton", "OK", parent_index=1),
                element(3, "AXButton", "OK"),
            ]
        )
        ids = [c.id for c in eligible_controls(observe(payload), "macos").controls]
        self.assertEqual(ids[0], "ax:button:options")
        self.assertTrue(ids[1].startswith("ax:button:ok:") and ids[2].startswith("ax:button:ok:"))
        self.assertNotEqual(ids[1], ids[2])
        twins = synthetic([element(1, "AXButton", "OK"), element(2, "AXButton", "OK")])
        twin_ids = [c.id for c in eligible_controls(observe(twins), "macos").controls]
        self.assertEqual(len(set(twin_ids)), 2)

    def test_slug_bounds(self) -> None:
        self.assertEqual(slug("Click target (left / right / double)"), "click-target-left-right-double")
        self.assertLessEqual(len(slug("x" * 200)), 32)
        self.assertRegex(slug("你好"), r"^[0-9a-f]{8}$")


class PolicyTest(unittest.TestCase):
    def test_risk_categories(self) -> None:
        self.assertEqual(risk_categories("Delete file"), {"destructive"})
        self.assertEqual(risk_categories("Don’t Save"), {"close_unsaved"})
        self.assertEqual(risk_categories("Buy now"), {"purchase"})
        self.assertEqual(risk_categories("Submit"), {"send"})
        self.assertEqual(risk_categories("Resetting"), frozenset())  # whole words only
        self.assertEqual(risk_categories("Save note"), frozenset())

    def test_compose_order_dedup_risk_and_cap(self) -> None:
        def cand(i: str, source: str, risk=frozenset()) -> Candidate:
            return Candidate(i, "d", "click", {}, source=source, risk=risk)

        ax = [cand(f"ax:button:b{n}", "ax") for n in range(30)]
        page = [cand("ax:button:b0", "page"), cand("p1", "page")]
        risky = [cand("ax:button:delete", "ax", frozenset({"destructive"}))]
        candidates, stats = compose(
            {"visual": [cand("v1", "visual")], "ax": risky + ax, "page": page},
            allowed_risks=frozenset(),
        )
        ids = [c.id for c in candidates]
        self.assertEqual(ids[:3], ["ax:button:b0", "p1", "ax:button:b1"])
        self.assertEqual(candidates[0].source, "page")  # first source wins
        self.assertEqual(ids[-2:], ["reobserve", "abstain"])
        self.assertEqual(len(ids), MAX_EXECUTABLE_CANDIDATES + 2)
        self.assertEqual(stats.duplicates, 1)
        self.assertEqual(stats.risk_excluded, {"destructive": 1})
        self.assertEqual(stats.dropped, 32 - MAX_EXECUTABLE_CANDIDATES)  # 2 page + 29 ax + 1 visual
        allowed, _ = compose({"ax": risky}, allowed_risks=frozenset({"destructive"}))
        self.assertIn("ax:button:delete", [c.id for c in allowed])


class AppKitTaskTest(unittest.TestCase):
    def sources(self, task, name: str = "initial", foreground=frozenset()) -> TaskSources:
        ax = NativeAccessibilitySource.from_observation(
            observe(window_state(name)), "macos", redact=task.redact_text, text_method=task.text_method
        )
        return TaskSources(ax=ax, foreground_ids=frozenset(foreground))

    def test_candidates_and_arguments(self) -> None:
        task = appkit_task("appkit-save-note", Path("/tmp/none.json"), note_text="secret note")
        step = task.plan(self.sources(task))
        by_id = {c.id: c for c in step.candidates}
        self.assertNotIn("ax:button:reset", by_id)
        self.assertNotIn("ax:button:exit", by_id)
        self.assertNotIn("ax:checkbox:i-agree", by_id)  # toggle not allowed
        setter = by_id["ax:text_input:note:set:note"]
        self.assertEqual(setter.tool, "set_value")
        self.assertEqual(setter.arguments["value"], "secret note")
        self.assertEqual(setter.arguments["element_token"], "s00000086:21")
        self.assertEqual(setter.snapshot_id, "s00000086")
        save = by_id["ax:button:save-note"]
        self.assertEqual(dict(save.arguments), {
            "pid": 20535, "window_id": 71364, "element_token": "s00000086:22",
            "delivery_mode": "background",
        })
        request = native_choice_request(task, self.sources(task), step, [])
        validate_request(request)
        wire = json.dumps(request)
        self.assertNotIn("secret note", wire)
        self.assertNotIn("element_token", wire)
        self.assertNotIn("s00000086:", wire)
        self.assertEqual(request["schema"], "cua.jev_choice_request_v2")
        self.assertEqual({c.get("source") for c in request["candidates"][:-2]}, {"ax"})

    def test_type_text_method(self) -> None:
        ax = NativeAccessibilitySource.from_observation(observe(window_state()), "macos", text_method="type_text")
        control = ax.find("text_input", "Note")
        candidate = ax.type_text(control, "hi", candidate_id="x", description="d")
        self.assertEqual(candidate.tool, "type_text")
        self.assertEqual(candidate.arguments["text"], "hi")

    def test_satisfied_controls_leave_the_set(self) -> None:
        note = appkit_task("appkit-save-note", Path("/tmp/none.json"), note_text="hello jev")
        ids = [c.id for c in note.plan(self.sources(note, "after-actions")).candidates]
        self.assertNotIn("ax:text_input:note:set:note", ids)
        size = appkit_task("appkit-choose-size", Path("/tmp/none.json"))
        ids = [c.id for c in size.plan(self.sources(size, "after-actions")).candidates]
        self.assertNotIn("ax:radio:large", ids)  # already selected
        self.assertIn("ax:radio:small", ids)

    def test_foreground_escalation_requires_permission(self) -> None:
        refused = {"ax:button:increment"}
        denied = appkit_task("appkit-counter", Path("/tmp/none.json"))
        ids = [c.id for c in denied.plan(self.sources(denied, foreground=refused)).candidates]
        self.assertNotIn("ax:button:increment", ids)
        self.assertNotIn("ax:button:increment:foreground", ids)
        allowed = appkit_task("appkit-counter", Path("/tmp/none.json"), allow_foreground=True)
        step = allowed.plan(self.sources(allowed, foreground=refused))
        fg = {c.id: c for c in step.candidates}["ax:button:increment:foreground"]
        self.assertEqual(fg.arguments["delivery_mode"], "foreground")

    def test_mock_follows_task_preferences(self) -> None:
        for task_id, expected in (
            ("appkit-counter", "ax:button:increment"),
            ("appkit-save-note", "ax:text_input:note:set:note"),
            ("appkit-choose-size", "ax:radio:large"),
        ):
            task = appkit_task(task_id, Path("/tmp/none.json"))
            sources = self.sources(task)
            choice, confidence, _ = choose_mock_for_task(task, sources, task.candidates(sources), [])
            self.assertEqual((choice, confidence), (expected, 1.0))
        size = appkit_task("appkit-choose-size", Path("/tmp/none.json"))
        sources = self.sources(size, "after-actions")
        choice, _, _ = choose_mock_for_task(size, sources, size.candidates(sources), [])
        self.assertEqual(choice, "ax:checkbox:i-agree")

    def test_oracle_and_classification(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "state.json"
            state = {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 3,
                     "agreed": True, "size": "large", "note_saved": "n"}
            path.write_text(json.dumps(state))
            counter = appkit_task("appkit-counter", path, pid=42)
            self.assertEqual(counter.classify(counter.read_oracle(), steps=3), "verified")
            self.assertEqual(appkit_task("appkit-save-note", path, note_text="n").classify(state, steps=2), "verified")
            self.assertEqual(appkit_task("appkit-save-note", path, note_text="m").classify(state, steps=2), "refuted")
            self.assertEqual(appkit_task("appkit-choose-size", path).classify(state, steps=2), "verified")
            self.assertEqual(counter.classify({**state, "counter": 4}, steps=4), "refuted")
            self.assertEqual(counter.classify({**state, "counter": 1}, steps=1), "unknown")
            self.assertEqual(counter.classify({**state, "counter": 1}, steps=6), "budget_exhausted")
            with self.assertRaises(OracleError):
                appkit_task("appkit-counter", path, pid=43).read_oracle()
            with self.assertRaises(OracleError):
                AppStateOracle(Path(directory) / "missing.json", "cua.appkit_task_state_v1").read()
        self.assertEqual(set(APPKIT_TASK_IDS), {"appkit-counter", "appkit-save-note", "appkit-choose-size"})

    def test_history_is_compact_and_redacted(self) -> None:
        task = appkit_task("appkit-save-note", Path("/tmp/none.json"), note_text="topsecret")
        entry = task.history_entry(2, "x", outcome="typed topsecret")
        self.assertEqual(entry["outcome"], "typed [note text]")
        self.assertIn("stale", task.history_entry(3, "x", stale=True)["outcome"])

    def test_visual_fallback_rule(self) -> None:
        task = appkit_task("appkit-counter", Path("/tmp/none.json"))
        visual_task = type(task)(**{
            **{k: getattr(task, k) for k in task.__dataclass_fields__ if k != "completion_candidate_ids"},
            "allowed_actions": frozenset({"press", "visual_click"}),
            "visual_targets": ("Increment",),
        })

        def reason(payload: dict, count: int) -> str | None:
            ax = NativeAccessibilitySource.from_observation(observe(payload), "macos")
            return visual_fallback_reason(TaskSources(ax=ax), visual_task, count)

        self.assertIsNone(reason(window_state(), 0))  # macOS never claims completeness
        self.assertIsNone(reason(synthetic([], truncated=True, elements_complete=True), 0))
        self.assertEqual(reason(synthetic([], degraded=True, degraded_reason="ax_tree_empty: none"), 0), "tree_empty")
        self.assertEqual(reason(synthetic([], elements_complete=True), 0), "no_native_candidates")
        complete = synthetic([element(1, "AXButton", "Other")], elements_complete=True)
        self.assertEqual(reason(complete, 1), "target_without_element")
        self.assertIsNone(visual_fallback_reason(
            TaskSources(ax=NativeAccessibilitySource.from_observation(observe(synthetic([], elements_complete=True)), "macos")),
            task, 0))  # visual_click not allowed

    def test_observation_rejects_other_windows(self) -> None:
        with self.assertRaises(ValueError):
            NativeObservation.from_window_state(window_state(), expected_pid=1, expected_window_id=2)


if __name__ == "__main__":
    unittest.main()
