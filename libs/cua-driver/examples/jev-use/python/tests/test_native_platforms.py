"""Native tasks on the WPF and WinUI3 (Windows UIA) and GTK3 (Linux AT-SPI) harnesses (RFC #4268).

The fixtures are real ``get_window_state`` results from each harness in task
mode, recorded by ``verify_native.py --capture-dir``.
"""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from choose_action import validate_request
from jev_adapter import choose_mock_for_task
from native import NativeObservation, eligible_controls
from native_roles import RAW_ROLES, role_class
from native_tasks import (
    HARNESSES,
    NATIVE_TASK_IDS,
    appkit_task,
    native_choice_request,
    native_task,
    split_task_id,
)
from sources import NativeAccessibilitySource
from tasks import TaskSources

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures/native"


def window_state(harness: str, name: str) -> dict:
    path = FIXTURES / f"{harness}-window-state-{name}-v1.json"
    return json.loads(path.read_text(encoding="utf-8"))


def sources(task, harness: str, name: str) -> TaskSources:
    payload = window_state(harness, name)
    observation = NativeObservation.from_window_state(
        payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
    )
    ax = NativeAccessibilitySource.from_observation(
        observation, HARNESSES[harness].platform, redact=task.redact_text, text_method=task.text_method
    )
    return TaskSources(ax=ax)


class HarnessRegistryTest(unittest.TestCase):
    def test_every_harness_has_the_same_three_tasks(self) -> None:
        self.assertEqual(
            NATIVE_TASK_IDS,
            (
                *(f"{h}-{k}" for h in ("appkit", "wpf", "winui3", "gtk3") for k in ("counter", "save-note", "choose-size")),
                "canvas-cancel",
            ),
        )
        for task_id in NATIVE_TASK_IDS:
            if task_id == "canvas-cancel":
                continue  # the visual-only canvas is not a form harness (see test_native_canvas.py)
            harness, kind = split_task_id(task_id)
            task = native_task(task_id, Path("/tmp/none.json"), pid=7)
            self.assertEqual(task.scope.window_title, harness.window_title)
            self.assertEqual(task.oracle.schema, harness.state_schema)
            self.assertEqual(task.oracle.expected_pid, 7)
            # Same semantics everywhere: the mock takes the same IDs on every platform.
            self.assertEqual(
                task.mock_preferences,
                native_task(f"appkit-{kind}", Path("/tmp/none.json")).mock_preferences,
            )

    def test_platforms_and_windows(self) -> None:
        self.assertEqual({h.name: h.platform for h in HARNESSES.values()},
                         {"appkit": "macos", "wpf": "windows", "winui3": "windows", "gtk3": "linux"})
        self.assertEqual(HARNESSES["wpf"].window_title, "CuaTestHarness WPF Tasks")
        self.assertEqual(HARNESSES["winui3"].window_title, "CuaTestHarness WinUI3 Tasks")
        self.assertEqual(HARNESSES["winui3"].state_env, "CUA_WINUI3_TASK_STATE")
        self.assertEqual(HARNESSES["winui3"].process_name, "CuaTestHarness.WinUI3")
        self.assertEqual(HARNESSES["gtk3"].window_title, "CuaTestHarness GTK3 Tasks")

    def test_unknown_tasks_are_rejected(self) -> None:
        for task_id in ("wpf-reset", "winui3-exit", "uwp-counter", "counter", ""):
            with self.assertRaises(ValueError):
                split_task_id(task_id)
        with self.assertRaises(ValueError):
            appkit_task("wpf-counter", Path("/tmp/none.json"))


class RecordedHarnessFixtureTest(unittest.TestCase):
    """Role table and element rules against real WPF, WinUI3, and GTK3 observations."""

    HARNESSES = ("wpf", "winui3", "gtk3")

    def test_task_controls_map_to_the_same_candidate_ids(self) -> None:
        for harness in self.HARNESSES:
            with self.subTest(harness=harness):
                note = native_task(f"{harness}-save-note", Path("/tmp/none.json"), note_text="secret note")
                ids = [c.id for c in note.plan(sources(note, harness, "initial")).candidates]
                self.assertEqual(ids[-2:], ["reobserve", "abstain"])
                self.assertIn("ax:text_input:note:set:note", ids)
                self.assertIn("ax:button:save-note", ids)
                self.assertIn("ax:button:increment", ids)
                # Reset (destructive) and Exit (close_unsaved) never reach the chooser.
                self.assertNotIn("ax:button:reset", ids)
                self.assertNotIn("ax:button:exit", ids)
                size = native_task(f"{harness}-choose-size", Path("/tmp/none.json"))
                size_ids = [c.id for c in size.plan(sources(size, harness, "initial")).candidates]
                for expected in ("ax:radio:small", "ax:radio:medium", "ax:radio:large", "ax:checkbox:i-agree"):
                    self.assertIn(expected, size_ids)

    def test_requests_are_valid_and_leak_no_secret_or_token(self) -> None:
        for harness in self.HARNESSES:
            with self.subTest(harness=harness):
                task = native_task(f"{harness}-save-note", Path("/tmp/none.json"), note_text="secret note")
                task_sources = sources(task, harness, "initial")
                step = task.plan(task_sources)
                setter = {c.id: c for c in step.candidates}["ax:text_input:note:set:note"]
                self.assertEqual(setter.tool, "set_value")
                self.assertEqual(setter.arguments["value"], "secret note")
                request = native_choice_request(task, task_sources, step, [])
                validate_request(request)
                wire = json.dumps(request)
                self.assertNotIn("secret note", wire)
                self.assertNotIn("element_token", wire)
                self.assertNotIn(f"{task_sources.ax.observation.snapshot_id}:", wire)

    def test_selected_state_is_read_from_the_platform(self) -> None:
        for harness in self.HARNESSES:
            with self.subTest(harness=harness):
                size = native_task(f"{harness}-choose-size", Path("/tmp/none.json"))
                initial = sources(size, harness, "initial")
                choice, _, _ = choose_mock_for_task(size, initial, size.candidates(initial), [])
                self.assertEqual(choice, "ax:radio:large")
                after = sources(size, harness, "after-choose-size")
                controls = {c.id: c for c in after.ax.controls}
                self.assertIs(controls["ax:radio:large"].selected, True)
                self.assertIs(controls["ax:checkbox:i-agree"].selected, True)
                ids = [c.id for c in size.candidates(after)]
                self.assertNotIn("ax:radio:large", ids)  # already selected
                self.assertIn("ax:radio:small", ids)

    def test_text_entry_value_is_never_a_label(self) -> None:
        for harness in self.HARNESSES:
            with self.subTest(harness=harness):
                note = native_task(f"{harness}-save-note", Path("/tmp/none.json"))
                after = sources(note, harness, "after-save-note")
                field = next(c for c in after.ax.controls if c.role_class == "text_input")
                self.assertEqual(field.label, "Note")
                ids = [c.id for c in note.candidates(after)]
                if harness == "gtk3":
                    # Known limitation of Cua Driver 0.30.2 (#4291): a named
                    # AT-SPI text field reports no value, so the written note
                    # is not observable and the write stays offered.
                    self.assertIn("limitation", window_state(harness, "after-save-note")["_fixture"])
                    self.assertIsNone(field.value)
                    self.assertIn("ax:text_input:note:set:note", ids)
                    continue
                self.assertEqual(field.value, "jev-use native note")
                self.assertNotIn("ax:text_input:note:set:note", ids)  # already holds the parameter

    def test_windows_title_bar_is_window_chrome(self) -> None:
        for harness in ("wpf", "winui3"):
            with self.subTest(harness=harness):
                payload = window_state(harness, "initial")
                observation = NativeObservation.from_window_state(
                    payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
                )
                native = eligible_controls(observation, "windows")
                labels = [c.label for c in native.controls]
                for chrome in ("System", "Minimize", "Maximize", "Close"):
                    self.assertNotIn(chrome, labels)
                self.assertEqual(native.excluded.get("window_chrome"), 4)
                self.assertEqual(
                    labels,
                    ["Increment", "Reset", "I agree", "Small", "Medium", "Large", "Note", "Save note", "Exit"],
                )

    def test_winui3_roles_map_through_the_windows_table(self) -> None:
        """WinUI3's automation peers report the same UIA control types as WPF.

        No WinUI3-specific row is needed: every raw role in the recorded
        WinUI3 trees is either a Windows role-table row or deliberately
        excluded. WinUI3 exposes its TextBlock as static ``Text`` (WPF's task
        window does not), which is an unknown role and never a candidate.
        """
        windows_rows = {raw for rows in RAW_ROLES["windows"].values() for raw in rows}
        for name in ("initial", "after-save-note", "after-choose-size"):
            with self.subTest(fixture=name):
                roles = {e["role"] for e in window_state("winui3", name)["elements"]}
                self.assertEqual(roles - windows_rows, {"Text", "TitleBar"})
                self.assertIsNone(role_class("Text", "windows"))
        payload = window_state("winui3", "initial")
        observation = NativeObservation.from_window_state(
            payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
        )
        native = eligible_controls(observation, "windows")
        # The static counter label and the title bar container are unknown
        # roles; the title bar's four buttons are window chrome.
        self.assertEqual(native.excluded, {"unknown_role": 2, "window_chrome": 4})
        self.assertEqual(
            [(c.role_class, c.label) for c in native.controls],
            [("button", "Increment"), ("button", "Reset"), ("checkbox", "I agree"),
             ("radio", "Small"), ("radio", "Medium"), ("radio", "Large"),
             ("text_input", "Note"), ("button", "Save note"), ("button", "Exit")],
        )
        self.assertEqual(
            [c.id for c in native.controls],
            [c.id for c in eligible_controls(
                NativeObservation.from_window_state(
                    window_state("wpf", "initial"),
                    expected_pid=window_state("wpf", "initial")["pid"],
                    expected_window_id=window_state("wpf", "initial")["window_id"],
                ),
                "windows",
            ).controls],
        )

    def test_linux_roles_map_to_role_classes(self) -> None:
        payload = window_state("gtk3", "initial")
        observation = NativeObservation.from_window_state(
            payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
        )
        native = eligible_controls(observation, "linux")
        self.assertEqual(native.excluded, {})
        self.assertEqual(
            [(c.role_class, c.label) for c in native.controls],
            [("button", "Increment"), ("button", "Reset"), ("checkbox", "I agree"),
             ("radio", "Small"), ("radio", "Medium"), ("radio", "Large"),
             ("text_input", "Note"), ("button", "Save note"), ("button", "Exit")],
        )

if __name__ == "__main__":
    unittest.main()
