"""Larger candidate sets, relevance capping, and accuracy measurement (#4312)."""

from __future__ import annotations

import ast
import contextlib
import io
import json
import re
import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXAMPLE = HERE.parents[1]
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(EXAMPLE))

import measure_native  # noqa: E402
from native import NativeObservation, risk_categories  # noqa: E402
from native_tasks import (  # noqa: E402
    HARNESSES,
    MAX_EXECUTABLE_CANDIDATES,
    TASK_KINDS,
    compose,
    native_task,
)
from sources import Candidate, NativeAccessibilitySource  # noqa: E402
from tasks import TaskSources  # noqa: E402

FIXTURES = EXAMPLE / "fixtures" / "native"
APPS = EXAMPLE.parents[1] / "tests" / "fixtures" / "apps"
DENSITY_HARNESSES = ("gtk3", "appkit", "wpf", "winui3")
TARGETS = {
    "counter": {"ax:button:increment"},
    "save-note": {"ax:text_input:note:set:note", "ax:button:save-note"},
    "choose-size": {"ax:radio:large", "ax:checkbox:i-agree"},
}


def load(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def plan_for(harness: str, kind: str, payload: dict, cap_order: str = "relevance"):
    task = replace(native_task(f"{harness}-{kind}", Path("/nonexistent")), cap_order=cap_order)
    observation = NativeObservation.from_window_state(
        payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
    )
    ax = NativeAccessibilitySource.from_observation(
        observation, HARNESSES[harness].platform, redact=task.redact_text, text_method=task.text_method
    )
    return task.plan(TaskSources(ax=ax))


class ComposeRelevanceTest(unittest.TestCase):
    @staticmethod
    def cand(name: str) -> Candidate:
        return Candidate(name, "d", "click", {}, source="ax")

    def test_within_cap_is_unchanged(self) -> None:
        group = [self.cand(f"ax:button:b{n}") for n in range(MAX_EXECUTABLE_CANDIDATES)]
        plain, _ = compose({"ax": group}, allowed_risks=frozenset())
        ranked, stats = compose(
            {"ax": group}, allowed_risks=frozenset(), relevance=lambda c: 0 if c.id.endswith("23") else 2
        )
        self.assertEqual([c.id for c in plain], [c.id for c in ranked])
        self.assertEqual(stats.dropped, 0)

    def test_over_cap_keeps_relevant_in_element_order(self) -> None:
        group = [self.cand(f"ax:button:b{n}") for n in range(30)]
        relevant = {"ax:button:b29", "ax:button:b27"}
        candidates, stats = compose(
            {"ax": group}, allowed_risks=frozenset(), relevance=lambda c: 0 if c.id in relevant else 2
        )
        ids = [c.id for c in candidates]
        self.assertEqual(len(ids), MAX_EXECUTABLE_CANDIDATES + 2)
        self.assertEqual(stats.dropped, 6)
        self.assertTrue(relevant <= set(ids))
        # Ranking decides only which survive; the order stays depth-first.
        executable = ids[:-2]
        self.assertEqual(executable, sorted(executable, key=lambda i: int(i.rsplit("b", 1)[1])))
        self.assertEqual(executable[:22], [f"ax:button:b{n}" for n in range(22)])
        self.assertEqual(ids[-2:], ["reobserve", "abstain"])
        # Deterministic: the same input gives the same set.
        again, _ = compose(
            {"ax": group}, allowed_risks=frozenset(), relevance=lambda c: 0 if c.id in relevant else 2
        )
        self.assertEqual(ids, [c.id for c in again])


class DensityFixtureTest(unittest.TestCase):
    def test_sizes_and_cap(self) -> None:
        expected_sizes = {
            ("gtk3", 12): {"counter": 12, "save-note": 14, "choose-size": 12},
            ("appkit", 12): {"counter": 14, "save-note": 16, "choose-size": 12},
            ("wpf", 12): {"counter": 12, "save-note": 14, "choose-size": 12},
            ("winui3", 12): {"counter": 12, "save-note": 14, "choose-size": 12},
            **{(harness, 24): dict.fromkeys(TASK_KINDS, 26) for harness in DENSITY_HARNESSES},
        }
        for (harness, density), sizes in expected_sizes.items():
            payload = load(f"{harness}-window-state-density-{density}-v1.json")
            for kind in TASK_KINDS:
                with self.subTest(harness=harness, density=density, kind=kind):
                    plan = plan_for(harness, kind, payload)
                    self.assertEqual(len(plan.candidates), sizes[kind])
                    self.assertTrue(TARGETS[kind] <= {c.id for c in plan.candidates})
                    self.assertEqual(plan.stats.dropped > 0, density == 24)

    def test_golden_sets(self) -> None:
        """The presented IDs match the golden file that the TypeScript test also checks."""
        golden = load("native-density-candidates-v1.json")["sets"]
        self.assertEqual(len(golden), len(DENSITY_HARNESSES) * 2 * 3 * 2)
        for harness in DENSITY_HARNESSES:
            for density in (12, 24):
                payload = load(f"{harness}-window-state-density-{density}-v1.json")
                for kind in TASK_KINDS:
                    for cap_order in ("relevance", "depth_first"):
                        plan = plan_for(harness, kind, payload, cap_order=cap_order)
                        self.assertEqual(
                            {"ids": [c.id for c in plan.candidates], "dropped": plan.stats.dropped},
                            golden[f"{harness}-{kind}-d{density}-{cap_order}"],
                        )

    def test_depth_first_drops_targets_behind_distractors(self) -> None:
        for harness in DENSITY_HARNESSES:
            payload = load(f"{harness}-window-state-density-24-v1.json")
            for kind in TASK_KINDS:
                with self.subTest(harness=harness, kind=kind):
                    plan = plan_for(harness, kind, payload, cap_order="depth_first")
                    self.assertEqual(len(plan.candidates), MAX_EXECUTABLE_CANDIDATES + 2)
                    self.assertFalse(TARGETS[kind] & {c.id for c in plan.candidates})

    def test_cap_order_is_irrelevant_within_the_cap(self) -> None:
        fixtures = {
            "appkit": "appkit-window-state-initial-v1.json",
            "wpf": "wpf-window-state-initial-v1.json",
            "winui3": "winui3-window-state-initial-v1.json",
            "gtk3": "gtk3-window-state-initial-v1.json",
        }
        for harness, name in fixtures.items():
            payload = load(name)
            for kind in TASK_KINDS:
                with self.subTest(harness=harness, kind=kind):
                    ranked = plan_for(harness, kind, payload)
                    plain = plan_for(harness, kind, payload, cap_order="depth_first")
                    self.assertEqual(
                        [(c.id, c.description, dict(c.arguments)) for c in ranked.candidates],
                        [(c.id, c.description, dict(c.arguments)) for c in plain.candidates],
                    )
        for harness in ("gtk3", "wpf", "winui3"):
            payload = load(f"{harness}-window-state-density-12-v1.json")
            for kind in TASK_KINDS:
                self.assertEqual(
                    [c.id for c in plan_for(harness, kind, payload).candidates],
                    [c.id for c in plan_for(harness, kind, payload, cap_order="depth_first").candidates],
                )

    def test_relevance_uses_no_values(self) -> None:
        payload = load("gtk3-window-state-density-24-v1.json")
        for element in payload["elements"]:
            if element.get("label") == "Search":
                element["value"] = "Increment Save note Large I agree"
        plan = plan_for("gtk3", "save-note", payload)
        self.assertNotIn("ax:text_input:search:set:note", [c.id for c in plan.candidates][:1])
        baseline = plan_for("gtk3", "save-note", load("gtk3-window-state-density-24-v1.json"))
        self.assertEqual([c.id for c in plan.candidates], [c.id for c in baseline.candidates])

    def test_invalid_cap_order(self) -> None:
        with self.assertRaises(ValueError):
            replace(native_task("gtk3-counter", Path("/nonexistent")), cap_order="random")  # type: ignore[arg-type]


class ExpectedNextTest(unittest.TestCase):
    def entry(self, task, candidate_id: str) -> dict:
        return task.history_entry(1, candidate_id, outcome="done")

    def test_counter(self) -> None:
        task = native_task("gtk3-counter", Path("/nonexistent"))
        history: list[dict] = []
        for _ in range(3):
            self.assertEqual(task.expected_next(history), ["ax:button:increment"])
            history.append(self.entry(task, "ax:button:increment"))
        self.assertEqual(task.expected_next(history), [])

    def test_ordered_and_unordered_steps(self) -> None:
        note = native_task("gtk3-save-note", Path("/nonexistent"))
        self.assertEqual(note.expected_next([]), ["ax:text_input:note:set:note"])
        self.assertEqual(
            note.expected_next([self.entry(note, "ax:text_input:note:set:note")]), ["ax:button:save-note"]
        )
        size = native_task("gtk3-choose-size", Path("/nonexistent"))
        self.assertEqual(size.expected_next([]), ["ax:radio:large", "ax:checkbox:i-agree"])
        # A refused or reobserve step performed nothing.
        refused = size.history_entry(1, "ax:radio:large", refusal="background_denied")
        self.assertEqual(size.expected_next([refused]), ["ax:radio:large", "ax:checkbox:i-agree"])
        self.assertEqual(
            size.expected_next([self.entry(size, "ax:radio:large:foreground")]), ["ax:checkbox:i-agree"]
        )


class DistractorLabelTest(unittest.TestCase):
    """The harnesses' distractor labels are benign and identical across platforms."""

    @staticmethod
    def gtk3_lists() -> dict[str, list]:
        tree = ast.parse((APPS / "linux/gtk3/main.py").read_text(encoding="utf-8"))
        values = {}
        for node in tree.body:
            if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name):
                name = node.targets[0].id
                if name.startswith("DISTRACTOR_") or name == "DENSITY_COUNTS":
                    values[name] = ast.literal_eval(node.value)
        return values

    @staticmethod
    def swift_list(source: str, name: str) -> list[str]:
        match = re.search(rf"let {name}\s*(?::[^=]+)?=\s*\[(.*?)\n\]", source, re.S) or re.search(
            rf"let {name}\s*=\s*\[([^\n]*)\]", source
        )
        assert match, name
        return re.findall(r'"([^"]+)"', match.group(1))

    @staticmethod
    def csharp_list(source: str, name: str) -> list[str]:
        match = re.search(rf"{name}\s*=\s*\{{(.*?)\}};", source, re.S)
        assert match, name
        return re.findall(r'"([^"]+)"', match.group(1))

    def test_labels(self) -> None:
        gtk3 = self.gtk3_lists()
        swift = (APPS / "macos/appkit/main.swift").read_text(encoding="utf-8")
        self.assertEqual(list(gtk3["DISTRACTOR_BUTTONS"]), self.swift_list(swift, "kDistractorButtons"))
        self.assertEqual(list(gtk3["DISTRACTOR_CHECKBOXES"]), self.swift_list(swift, "kDistractorCheckboxes"))
        self.assertEqual(
            [label for group in gtk3["DISTRACTOR_RADIO_GROUPS"] for label in group],
            self.swift_list(swift, "kDistractorRadioGroups"),
        )
        self.assertEqual(list(gtk3["DISTRACTOR_FIELDS"]), self.swift_list(swift, "kDistractorFields"))
        self.assertIn("12: (8, 3, 1, 1), 24: (26, 12, 4, 2)", swift)
        self.assertEqual(gtk3["DENSITY_COUNTS"], {12: (8, 3, 1, 1), 24: (26, 12, 4, 2)})
        for harness in ("wpf", "winui3"):
            with self.subTest(harness=harness):
                source = (APPS / f"windows/{harness}/TaskWindow.cs").read_text(encoding="utf-8")
                self.assertEqual(list(gtk3["DISTRACTOR_BUTTONS"]), self.csharp_list(source, "DistractorButtons"))
                self.assertEqual(list(gtk3["DISTRACTOR_CHECKBOXES"]), self.csharp_list(source, "DistractorCheckboxes"))
                self.assertEqual(
                    [label for group in gtk3["DISTRACTOR_RADIO_GROUPS"] for label in group],
                    self.csharp_list(source, "DistractorRadioGroups"),
                )
                self.assertEqual(list(gtk3["DISTRACTOR_FIELDS"]), self.csharp_list(source, "DistractorFields"))
                self.assertIn("[12] = (8, 3, 1, 1), [24] = (26, 12, 4, 2)", source)
                self.assertIn(f'"CUA_{harness.upper()}_TASK_DENSITY"', source)
        labels = [
            *gtk3["DISTRACTOR_BUTTONS"], *gtk3["DISTRACTOR_CHECKBOXES"], *gtk3["DISTRACTOR_FIELDS"],
            *(label for group in gtk3["DISTRACTOR_RADIO_GROUPS"] for label in group),
        ]
        self.assertEqual(len(labels), len(set(labels)))
        task_labels = {"Increment", "Reset", "I agree", "Small", "Medium", "Large", "Note", "Save note", "Exit"}
        for label in labels:
            with self.subTest(label=label):
                self.assertEqual(risk_categories(label), frozenset())
                self.assertNotIn(label, task_labels)


class MeasureTest(unittest.TestCase):
    def test_bucket_and_classify(self) -> None:
        self.assertEqual([measure_native.bucket(n) for n in (4, 8, 9, 14, 18, 19, 26)],
                         ["~4", "~4", "~12", "~12", "~12", "~24", "~24"])
        self.assertEqual(measure_native.classify("ax:a:foreground", ["ax:a"]), "correct")
        self.assertEqual(measure_native.classify("ax:b", ["ax:a"]), "wrong_action")
        self.assertEqual(measure_native.classify("reobserve", ["ax:a"]), "reobserve")
        self.assertEqual(measure_native.classify(None, ["ax:a"]), "abstain")
        self.assertEqual(measure_native.classify("ax:a", ["ax:a"], error=True), "error")

    def test_mock_replay(self) -> None:
        with tempfile.TemporaryDirectory() as out:
            specs = [
                f"gtk3:0:{FIXTURES / 'gtk3-window-state-initial-v1.json'}",
                f"gtk3:12:{FIXTURES / 'gtk3-window-state-density-12-v1.json'}",
                f"gtk3:24:{FIXTURES / 'gtk3-window-state-density-24-v1.json'}",
            ]
            args = ["replay", "--provider", "mock", "--reps", "2", "--cap-order", "depth_first",
                    "--cap-order", "relevance", "--order", "element", "--order", "shuffled", "--out", out,
                    "--group-by", "density,cap_order,order,bucket"]
            for spec in specs:
                args += ["--fixture", spec]
            with contextlib.redirect_stdout(io.StringIO()):
                measure_native.main(args)
            rows = json.loads((Path(out) / "table.json").read_text())
            decisions = [json.loads(line) for line in (Path(out) / "decisions.jsonl").read_text().splitlines()]
        # Seven decision points per repetition: counter 3, save-note 2, choose-size 2.
        self.assertEqual(len(decisions), 3 * 2 * 2 * 2 * 7)
        for row in rows:
            with self.subTest(row=row):
                if row["density"] == 24 and row["cap_order"] == "depth_first":
                    self.assertEqual((row["correct"], row["target_missing"]), (0, row["decisions"]))
                    self.assertEqual(row["reobserve"], row["decisions"])
                else:
                    self.assertEqual(row["accuracy"], 1.0)
                    self.assertEqual(row["target_missing"], 0)
        self.assertEqual({row["bucket"] for row in rows}, {"~4", "~12", "~24"})
        shuffled = [d for d in decisions if d["order"] == "shuffled" and d["density"] == 24]
        element = [d for d in decisions if d["order"] == "element" and d["density"] == 24]
        self.assertNotEqual({d["request_sha256"] for d in shuffled}, {d["request_sha256"] for d in element})

    def test_runner_logs(self) -> None:
        events = [
            {"event": "start", "task": "gtk3-counter", "language": "python", "provider": "s1", "platform": "linux"},
            {"event": "step", "step": 1, "candidate": "ax:button:increment", "candidate_count": 26,
             "expected_ids": ["ax:button:increment"], "expected_offered": True, "confidence": 0.9,
             "decide_ms": 2000.0, "compose": {"dropped": 4}},
            {"event": "step", "step": 2, "candidate": "ax:button:refresh", "candidate_count": 26,
             "expected_ids": ["ax:button:increment"], "expected_offered": True, "confidence": 0.4,
             "decide_ms": 2100.0},
            {"event": "step", "step": 3, "candidate": "reobserve", "candidate_count": 26,
             "expected_ids": ["ax:button:increment"], "expected_offered": True, "confidence": 0.5,
             "decide_ms": 2050.0},
            {"event": "outcome", "outcome": "abstained", "step": 4, "candidate_count": 26,
             "expected_ids": ["ax:button:increment"], "confidence": 0.6, "decide_ms": 1990.0},
            {"event": "outcome", "outcome": "unknown", "phase": "decide", "step": 5, "candidate_count": 26,
             "expected_ids": ["ax:button:increment"], "error": "S1ServiceError"},
            {"event": "outcome", "outcome": "verified", "step": 5},
        ]
        with tempfile.TemporaryDirectory() as root:
            log = Path(root) / "python-s1-gtk3-counter-d24.jsonl"
            log.write_text("".join(json.dumps(event) + "\n" for event in events))
            records = measure_native.decisions_from_log(log)
        self.assertEqual([r["result"] for r in records], ["correct", "wrong_action", "reobserve", "abstain", "error"])
        self.assertEqual({r["density"] for r in records}, {24})
        self.assertEqual({r["bucket"] for r in records}, {"~24"})
        rows = measure_native.aggregate(records, ("platform", "provider", "bucket"))
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["accuracy"], 0.2)
        self.assertIn("| linux | s1 | ~24 | 26 | 5 | 1 | 20.0% |", measure_native.markdown(rows, ("platform", "provider", "bucket")))


if __name__ == "__main__":
    unittest.main()
