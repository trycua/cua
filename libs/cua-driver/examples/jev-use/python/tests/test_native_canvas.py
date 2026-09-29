"""The visual fallback for views without application accessibility elements.

The recorded fixtures are real ``get_window_state`` results for the
cross-platform visual-only canvas (a custom-painted Tk surface): on Linux X11,
Driver recovers only window metadata; on Windows, UIA exposes only the title
bar; on macOS, AX exposes only the two unlabeled window buttons and the
application menu bar. None holds an application element, so the canvas task may parse
visual regions from the same capture, and its only executable candidate is a
capture-bound visual click.
"""

from __future__ import annotations

import json
import sys
from dataclasses import replace
import tempfile
import unittest
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from choose_action import validate_request
from core import VisualObservation, VisualRegion
from jev_adapter import choose_mock_for_task
from native import NativeObservation, has_application_elements
from native_tasks import (
    CANVAS,
    CANVAS_TASK_ID,
    HARNESSES,
    NATIVE_TASK_IDS,
    native_choice_request,
    native_task,
    split_task_id,
    visual_fallback_reason,
)
from sources import NativeAccessibilitySource, VisualRegionSource
from tasks import TaskSources

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures/native"
CANVAS_FIXTURES = {"linux": "canvas-linux", "windows": "canvas-windows", "macos": "canvas-macos"}


def observation(name: str) -> NativeObservation:
    payload = json.loads((FIXTURES / f"{name}-window-state-initial-v1.json").read_text(encoding="utf-8"))
    return NativeObservation.from_window_state(
        payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
    )


def canvas_sources(platform: str, *, visual: bool = True, foreground_ids=frozenset()) -> TaskSources:
    observed = observation(CANVAS_FIXTURES[platform])
    ax = NativeAccessibilitySource.from_observation(observed, platform)
    if not visual:
        return TaskSources(ax=ax, foreground_ids=foreground_ids)
    regions = tuple(
        VisualRegion(f"r{index}", "text", text, None, 0.95, False, x, 250, 80, 30)
        for index, (text, x) in enumerate((("Save", 130), ("Send", 350), ("Cancel", 570)))
    )
    regions += (VisualRegion("r9", "text", "CHOOSE A SIGNAL", None, 0.95, False, 40, 30, 300, 30),)
    parsed = VisualObservation(
        observed.capture_id or "", "ref", 762, 492, observed.pid, observed.window_id,
        (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), regions,
    )
    return TaskSources(
        ax=ax,
        visual=VisualRegionSource(parsed, "background", True),
        visual_path=True,
        foreground_ids=foreground_ids,
    )


class ApplicationElementsTest(unittest.TestCase):
    def test_recorded_canvas_trees_have_no_application_elements(self) -> None:
        for platform, name in CANVAS_FIXTURES.items():
            with self.subTest(platform=platform):
                observed = observation(name)
                self.assertFalse(observed.complete)
                self.assertFalse(observed.truncated)
                self.assertFalse(observed.tree_empty)  # Driver does not report ax_tree_empty
                self.assertFalse(has_application_elements(observed, platform))
                ax = NativeAccessibilitySource.from_observation(observed, platform)
                self.assertEqual(ax.controls, ())

    def test_recorded_harness_trees_have_application_elements(self) -> None:
        for harness in ("appkit", "wpf", "winui3", "gtk3"):
            with self.subTest(harness=harness):
                self.assertTrue(has_application_elements(observation(harness), HARNESSES[harness].platform))

    def test_windows_title_bar_counts_as_chrome_only_on_windows(self) -> None:
        # Linux has no chrome rule, so the same buttons count as application elements.
        self.assertTrue(has_application_elements(observation("canvas-windows"), "linux"))

    def test_macos_menu_bar_and_window_buttons_are_not_content(self) -> None:
        payload = json.loads((FIXTURES / "canvas-macos-window-state-initial-v1.json").read_text(encoding="utf-8"))
        # The menu bar and unlabeled window buttons count as content on other platforms.
        self.assertTrue(has_application_elements(observation("canvas-macos"), "linux"))
        cases = {
            "labeled window button": {"element_index": 90, "parent_index": 0, "role": "AXButton", "label": "Save"},
            "unlabeled nested button": {"element_index": 91, "parent_index": 92, "role": "AXButton"},
            "group": {"element_index": 92, "parent_index": 0, "role": "AXGroup"},
        }
        for name, extra in cases.items():
            with self.subTest(name=name):
                elements = payload["elements"] + [extra]
                if name == "unlabeled nested button":
                    elements.append(cases["group"])
                observed = NativeObservation.from_window_state(
                    {**payload, "elements": elements},
                    expected_pid=payload["pid"], expected_window_id=payload["window_id"],
                )
                self.assertTrue(has_application_elements(observed, "macos"))


class FallbackRuleTest(unittest.TestCase):
    def test_canvas_trees_fall_back_to_visual_regions(self) -> None:
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        for platform in CANVAS_FIXTURES:
            with self.subTest(platform=platform):
                sources = canvas_sources(platform, visual=False)
                self.assertEqual(visual_fallback_reason(sources, task, 0), "no_application_elements")

    def test_partial_tree_with_application_elements_never_falls_back(self) -> None:
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        observed = observation("appkit")  # macOS never claims completeness
        ax = NativeAccessibilitySource.from_observation(observed, "macos")
        self.assertIsNone(visual_fallback_reason(TaskSources(ax=ax), task, 0))

    def test_truncated_or_uncaptured_trees_never_fall_back(self) -> None:
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        payload = json.loads((FIXTURES / "canvas-linux-window-state-initial-v1.json").read_text(encoding="utf-8"))
        for change in ({"truncated": True}, {"capture_id": None}):
            with self.subTest(change=change):
                observed = NativeObservation.from_window_state(
                    {**payload, **change}, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
                )
                ax = NativeAccessibilitySource.from_observation(observed, "linux")
                self.assertIsNone(visual_fallback_reason(TaskSources(ax=ax), task, 0))

    def test_form_tasks_never_parse_visual_regions(self) -> None:
        task = native_task("gtk3-counter", Path("/tmp/none.json"))
        self.assertIsNone(visual_fallback_reason(canvas_sources("linux", visual=False), task, 0))


class CanvasTaskTest(unittest.TestCase):
    def test_scope_and_confidence(self) -> None:
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        self.assertEqual(task.scope.window_state_arguments(), {"max_depth": 1})
        self.assertEqual(task.visual_min_confidence, 0.8)
        form = native_task("appkit-counter", Path("/tmp/none.json"))
        self.assertEqual(form.scope.window_state_arguments(), {})
        self.assertEqual(form.visual_min_confidence, 0.8)
        sources = canvas_sources("macos")
        low = [replace(region, confidence=0.75) for region in sources.visual.observation.regions]
        observed = replace(sources.visual.observation, regions=tuple(low))
        self.assertIsNone(VisualRegionSource(observed, "background", True).find("button", "Save"))
        self.assertIsNotNone(VisualRegionSource(observed, "background", True, 0.7).find("button", "Save"))

    def test_registry(self) -> None:
        self.assertIn(CANVAS_TASK_ID, NATIVE_TASK_IDS)
        self.assertEqual(split_task_id(CANVAS_TASK_ID), (CANVAS, "cancel"))
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"), pid=7)
        self.assertEqual(task.scope.window_title, "Cua Visual-Only Canvas Fixture")
        self.assertEqual(task.oracle.schema, "cua.visual_canvas_task_state_v1")
        self.assertEqual(task.allowed_action_kinds, frozenset({"click"}))

    def test_only_the_cancel_region_is_executable(self) -> None:
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        for platform in CANVAS_FIXTURES:
            with self.subTest(platform=platform):
                sources = canvas_sources(platform)
                step = task.plan(sources)
                self.assertEqual([c.id for c in step.candidates], ["visual:cancel", "reobserve", "abstain"])
                save = step.candidates[0]
                self.assertEqual(save.source, "visual")
                self.assertEqual(save.tool, "click")
                self.assertEqual(save.arguments["delivery_mode"], "background")
                self.assertEqual(save.arguments["capture_id"], sources.ax.observation.capture_id)
                self.assertEqual((save.arguments["x"], save.arguments["y"]), (610.0, 265.0))
                request = native_choice_request(task, sources, step, [])
                validate_request(request)
                self.assertEqual([c["source"] for c in request["candidates"][:1]], ["visual"])
                self.assertEqual(request["elements"], [])
                choice, _, _ = choose_mock_for_task(task, sources, step.candidates, [])
                self.assertEqual(choice, "visual:cancel")

    def test_background_refusal_offers_an_explicit_foreground_variant(self) -> None:
        refused = frozenset({"visual:cancel"})
        allowed = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"), allow_foreground=True)
        step = allowed.plan(canvas_sources("linux", foreground_ids=refused))
        self.assertEqual([c.id for c in step.candidates], ["visual:cancel:foreground", "reobserve", "abstain"])
        self.assertEqual(step.candidates[0].arguments["delivery_mode"], "foreground")
        self.assertEqual(
            choose_mock_for_task(allowed, canvas_sources("linux", foreground_ids=refused), step.candidates, [])[0],
            "visual:cancel:foreground",
        )
        denied = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        self.assertEqual(
            [c.id for c in denied.plan(canvas_sources("linux", foreground_ids=refused)).candidates],
            ["reobserve", "abstain"],
        )

    def test_oracle(self) -> None:
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        self.assertEqual(task.check({"selected": None, "action_count": 0}), "pending")
        self.assertEqual(task.check({"selected": "cancel", "action_count": 1}), "verified")
        self.assertEqual(task.check({"selected": "cancel", "action_count": 2}), "refuted")
        self.assertEqual(task.check({"selected": "save", "action_count": 1}), "refuted")
        self.assertEqual(task.check({"selected": "send", "action_count": 1}), "refuted")


class JournalTest(unittest.TestCase):
    def test_journal_writes_each_published_state_with_the_task_schema(self) -> None:
        import verify_native

        with tempfile.TemporaryDirectory() as work:
            state = Path(work) / "state.json"
            url, close = verify_native.start_journal(verify_native.HARNESSES["canvas"], state)
            try:
                for published in ({"pid": 5, "selected": None, "action_count": 0},
                                  {"pid": 5, "selected": "save", "action_count": 1}):
                    request = urllib.request.Request(
                        url, data=json.dumps(published).encode(), method="POST",
                        headers={"Content-Type": "application/json"},
                    )
                    with urllib.request.urlopen(request, timeout=5) as response:
                        self.assertEqual(response.status, 204)
                    written = json.loads(state.read_text(encoding="utf-8"))
                    self.assertEqual(written, {**published, "schema": "cua.visual_canvas_task_state_v1"})
            finally:
                close()
        self.assertTrue(url.startswith("http://127.0.0.1:"))


if __name__ == "__main__":
    unittest.main()
