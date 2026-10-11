from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import synth
from synth import C

PROBES = synth.PROBES
SWIFT_DIR = PROBES.parent / "swift"
PROBE_MODES = {
    "PROBE-FORMS": "forms",
    "PROBE-TABLE": "table",
    "PROBE-CANVAS": "canvas",
    "PROBE-CANVASCLICK": "canvasclick",
    "PROBE-HOVER": "hover",
    "PROBE-CLIPBOARD": "clipboard",
}
RUNS = {
    "forms": synth.forms_run,
    "table": synth.table_run,
    "canvas": synth.canvas_run,
    "canvasclick": synth.canvasclick_run,
    "hover": synth.hover_run,
    "clipboard": synth.clipboard_run,
}


class PrngTests(unittest.TestCase):
    def test_splitmix64_reference_vector(self):
        # Published splitmix64 outputs for seed 0.
        r = C.SplitMix64(0)
        self.assertEqual(r.next(), 0xE220A8397B1DCDAF)
        self.assertEqual(r.next(), 0x6E789E6AA1B965F4)
        self.assertEqual(r.next(), 0x06C45D188009454F)

    def test_seed_wraps_to_64_bits(self):
        self.assertEqual(C.SplitMix64(2**64 + 5).next(), C.SplitMix64(5).next())

    def test_derivations_are_deterministic_and_seed_sensitive(self):
        self.assertEqual(C.derive_all(123), C.derive_all(123))
        self.assertNotEqual(C.derive_forms(1), C.derive_forms(2))

    def test_randint_and_shuffle_ranges(self):
        r = C.SplitMix64(9)
        values = [r.randint(3, 6) for _ in range(500)]
        self.assertEqual(set(values), {3, 4, 5, 6})
        self.assertEqual(sorted(r.shuffled([0, 1, 2, 3, 4])), [0, 1, 2, 3, 4])

    def test_forms_derivation_constraints(self):
        for seed in range(500):
            d = C.derive_forms(seed)
            self.assertIn(d["customer_name"], C.NAMES)
            self.assertRegex(d["invoice_amount"], r"^\d{3,4}\.(00|25|50|75)$")
            self.assertTrue(200 <= float(d["invoice_amount"]) < 9801)
            self.assertTrue(1 <= d["category_index"] <= 7)  # never the default selection
            self.assertEqual(d["category_title"], C.CATEGORIES[d["category_index"]])
            self.assertIn(d["priority"], C.PRIORITIES)
            self.assertTrue(2 <= d["quantity"] <= 99)
            self.assertIn(d["notes"], C.NOTES)

    def test_table_sizes(self):
        self.assertEqual(len(C.NAMES), 20)
        self.assertEqual(len(C.CATEGORIES), 8)
        self.assertEqual(len(C.NOTES), 10)
        self.assertEqual(len(set(C.NAMES)), 20)
        self.assertEqual(len(set(C.NOTES)), 10)


class SwiftSourceTests(unittest.TestCase):
    """Cheap drift check between Swift and Python tables; see test_swift_parity for the full one."""

    @classmethod
    def setUpClass(cls):
        cls.source = (SWIFT_DIR / "BenchLab.swift").read_text("utf-8")

    def swift_list(self, name):
        match = re.search(rf"static let {name} = \[(.*?)\]\n", self.source, re.S)
        self.assertIsNotNone(match, name)
        return re.findall(r'"([^"]*)"', match.group(1))

    def test_string_tables_match(self):
        self.assertEqual(self.swift_list("names"), C.NAMES)
        self.assertEqual(self.swift_list("categories"), C.CATEGORIES)
        self.assertEqual(self.swift_list("priorities"), C.PRIORITIES)
        self.assertEqual(self.swift_list("notes"), C.NOTES)
        self.assertEqual(self.swift_list("hoverNames"), C.HOVER_NAMES)
        self.assertEqual(self.swift_list("colors"), C.COLORS)

    def test_constants_match(self):
        def const(name):
            match = re.search(rf"^let {name}(?:: CGFloat)? = (\d+)", self.source, re.M)
            self.assertIsNotNone(match, name)
            return int(match.group(1))

        self.assertEqual(const("tableRows"), C.TABLE_ROWS)
        self.assertEqual(const("canvasW"), C.CANVAS_W)
        self.assertEqual(const("canvasH"), C.CANVAS_H)
        self.assertEqual(const("zoneSize"), C.ZONE_SIZE)
        self.assertEqual(const("tileSize"), C.TILE_SIZE)
        self.assertEqual(const("columnW"), C.COLUMN_W)
        self.assertEqual(const("clickCount"), C.CLICK_COUNT)
        self.assertEqual(const("clickCols"), C.CLICK_COLS)
        self.assertEqual(const("clickCellW"), C.CLICK_CELL_W)
        self.assertEqual(const("clickCellH"), C.CLICK_CELL_H)
        self.assertEqual(const("circleRadius"), C.CIRCLE_RADIUS)

    def test_randint_ranges_match(self):
        """The draw order and ranges are hard to diff textually; pin the literal calls."""
        for snippet in (
            "r.randint(200, 9800)",
            "[0, 25, 50, 75][r.index(4)]",
            "r.randint(1, 7)",
            "r.randint(2, 99)",
            "r.randint(30, 130)",
            "r.randint(131, 260)",
            "r.randint(261, 400)",
            "r.randint(8, 129)",
            "r.randint(16, 150)",
            "r.randint(8, 169)",
            "r.randint(310, 356)",
            "r.randint(-30, 30)",
            "r.randint(-26, 26)",
            "(i / clickCols) * clickCellH + 52 + jy",
            "r.randint(0, 3)",
            "r.randint(120, 989)",
            "r.randint(11, 97)",
            "r.randint(1000, 99999)",
        ):
            self.assertIn(snippet, self.source)


@unittest.skipUnless(
    os.environ.get("BENCHLAB_PARITY") == "1", "set BENCHLAB_PARITY=1 to compile Swift and compare"
)
class SwiftParityTests(unittest.TestCase):
    def test_swift_and_python_derive_identical_parameters(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "BenchLabDump"
            subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    "-Onone",
                    "-D",
                    "BENCHLAB_DUMP",
                    "-parse-as-library",
                    "-swift-version",
                    "5",
                    str(SWIFT_DIR / "BenchLab.swift"),
                    "-o",
                    str(binary),
                ],
                check=True,
            )
            for seed in [
                0,
                1,
                2,
                7,
                42,
                999_999,
                2**40 + 5,
                2**63 + 17,
                2**64 - 1,
                *range(100, 140),
            ]:
                out = subprocess.run(
                    [str(binary), "--dump-params", "--seed", str(seed)],
                    capture_output=True,
                    text=True,
                    check=True,
                )
                self.assertEqual(json.loads(out.stdout), C.derive_all(seed), seed)


class BriefTests(unittest.TestCase):
    FORBIDDEN = re.compile(
        r"evaluat|state file|events? file|accessib|\bAX\b|aria|cua|codex|driver|json|jsonl|seed|\{\{|\}\}|sentinel",
        re.I,
    )

    def render(self, probe_id, seed):
        return synth.load_module(probe_id, "render_brief.py").render(seed)

    def test_every_probe_renders_without_leaking_internals(self):
        for probe_id in PROBE_MODES:
            for seed in (1, 77, 999_999):
                text = self.render(probe_id, seed)
                self.assertIn("BenchLab", text)
                self.assertIsNone(
                    self.FORBIDDEN.search(text), (probe_id, self.FORBIDDEN.search(text))
                )

    def test_forms_brief_contains_every_expected_value(self):
        seed = 42
        p = C.derive_forms(seed)
        text = self.render("PROBE-FORMS", seed)
        for value in (
            p["customer_name"],
            p["invoice_amount"],
            p["category_title"],
            p["priority"],
            str(p["quantity"]),
            p["notes"],
        ):
            self.assertIn(value, text)
        self.assertIn("tick" if p["notify"] else "unticked", text)

    def test_table_brief_names_the_three_codes(self):
        text = self.render("PROBE-TABLE", 42)
        for code in C.derive_table(42)["codes"]:
            self.assertIn(code, text)

    def test_canvasclick_brief_lists_the_numbers_in_order(self):
        p = C.derive_canvasclick(42)
        text = self.render("PROBE-CANVASCLICK", 42)
        self.assertIn(", ".join(str(n) for n in p["left_sequence"]), text)
        self.assertIn(", ".join(str(n) for n in p["right_targets"]), text)
        self.assertIn("right-click", text)
        self.assertIn("Done", text)

    def test_hover_brief_names_the_target(self):
        text = self.render("PROBE-HOVER", 42)
        self.assertIn(f'"{C.derive_hover(42)["target"]}"', text)

    def test_clipboard_brief_gives_operands_but_not_the_answer(self):
        seed = 42
        p = C.derive_clipboard(seed)
        text = self.render("PROBE-CLIPBOARD", seed)
        for n in (p["a"], p["b"], p["c"]):
            self.assertIn(str(n), text)
        self.assertNotIn(str(p["result"]), text)
        self.assertIn("Calculator", text)

    def test_unknown_or_unused_placeholders_are_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "brief.md"
            path.write_text("hello {{a}} {{b}}", "utf-8")
            with self.assertRaises(KeyError):
                C.render_template(path, {"a": 1})
            with self.assertRaises(KeyError):
                C.render_template(path, {"a": 1, "b": 2, "c": 3})
            self.assertEqual(C.render_template(path, {"a": 1, "b": 2}), "hello 1 2")


class TaskJsonTests(unittest.TestCase):
    def test_task_json_shape(self):
        expected_tags = {
            "PROBE-FORMS": ["native_ax", "text_entry"],
            "PROBE-TABLE": ["native_ax", "scroll_virtualized"],
            "PROBE-CANVAS": ["pixel_drag", "no_ax"],
            "PROBE-CANVASCLICK": ["pixel_click", "no_ax"],
            "PROBE-HOVER": ["hover", "coverage_probe"],
            "PROBE-CLIPBOARD": ["multi_app", "cross_app_transfer"],
        }
        timeouts = {
            "PROBE-FORMS": 300,
            "PROBE-TABLE": 300,
            "PROBE-CANVAS": 300,
            "PROBE-CANVASCLICK": 360,
            "PROBE-HOVER": 240,
            "PROBE-CLIPBOARD": 300,
        }
        for probe_id, mode in PROBE_MODES.items():
            data = json.loads((PROBES / probe_id / "task.json").read_text("utf-8"))
            self.assertEqual(data["id"], probe_id)
            self.assertTrue(data["title"])
            self.assertEqual(data["dimension_tags"], expected_tags[probe_id])
            self.assertEqual(data["timeout_s"], timeouts[probe_id])
            self.assertEqual(data["app_args"], ["--mode", mode])
            for filename in ("evaluate.py", "brief.md", "render_brief.py"):
                self.assertTrue((PROBES / probe_id / filename).is_file())


class CliTests(unittest.TestCase):
    def run_cli(self, probe_id, seed, state_path, events_path, result_path):
        return subprocess.run(
            [
                sys.executable,
                str(PROBES / probe_id / "evaluate.py"),
                "--seed",
                str(seed),
                "--state",
                str(state_path),
                "--events",
                str(events_path),
                "--result",
                str(result_path),
            ],
            capture_output=True,
            text=True,
        )

    def result_of(self, tmp, probe_id, seed, state, events, torn=False, state_text=None):
        d = Path(tmp)
        state_path, events_path = synth.write_files(d, state, events, torn=torn)
        if state_text is not None:
            state_path.write_text(state_text, "utf-8")
        out = d / "out" / "result.json"
        proc = self.run_cli(probe_id, seed, state_path, events_path, out)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return json.loads(out.read_text("utf-8"))

    def test_good_run_writes_a_passing_result_for_every_probe(self):
        for probe_id, mode in PROBE_MODES.items():
            seed = 31
            state, events = RUNS[mode](seed)
            with tempfile.TemporaryDirectory() as tmp:
                result = self.result_of(tmp, probe_id, seed, state, events)
            self.assertTrue(result["passed"], (probe_id, result["checks"]))
            self.assertEqual(result["score"], 1.0)
            self.assertIsInstance(result["checks"]["integrity"]["weight"], float)
            for check in result["checks"].values():
                self.assertEqual(set(check), {"passed", "weight", "detail"})

    def test_missing_files_fail_closed_for_every_probe(self):
        for probe_id, mode in PROBE_MODES.items():
            seed = 31
            state, events = RUNS[mode](seed)
            for kind in ("state", "events", "both"):
                with tempfile.TemporaryDirectory() as tmp:
                    result = self.result_of(
                        tmp,
                        probe_id,
                        seed,
                        None if kind in ("state", "both") else state,
                        None if kind in ("events", "both") else events,
                    )
                self.assertFalse(result["passed"], (probe_id, kind))
                self.assertEqual(result["score"], 0.0)
                self.assertIn("inputs_loadable", result["checks"])

    def test_malformed_inputs_fail_closed(self):
        probe_id, seed = "PROBE-CLIPBOARD", 31
        state, events = synth.clipboard_run(seed)
        with tempfile.TemporaryDirectory() as tmp:
            for text in ("{not json", "[1, 2]", "", "null"):
                result = self.result_of(tmp, probe_id, seed, state, events, state_text=text)
                self.assertFalse(result["passed"], text)
        with tempfile.TemporaryDirectory() as tmp:
            result = self.result_of(tmp, probe_id, seed, state, events, torn=True)
            self.assertFalse(result["passed"])
            self.assertFalse(result["checks"]["integrity"]["passed"])
        with tempfile.TemporaryDirectory() as tmp:
            result = self.result_of(tmp, probe_id, seed, state, [])
            self.assertFalse(result["passed"])

    def test_wrong_probe_state_fails(self):
        seed = 31
        state, events = synth.table_run(seed)
        with tempfile.TemporaryDirectory() as tmp:
            result = self.result_of(tmp, "PROBE-CLIPBOARD", seed, state, events)
        self.assertFalse(result["passed"])


if __name__ == "__main__":
    unittest.main()
