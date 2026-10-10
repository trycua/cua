"""Bench v2 (Amendment 14): categories, GUI-only guard, background score, v2 task selection."""

from __future__ import annotations

import json
import sys
import tempfile
import threading
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import bench_v2 as v2  # noqa: E402
import run_bench as rb  # noqa: E402


def specs() -> dict[str, dict]:
    return {
        json.loads(p.read_text())["id"]: json.loads(p.read_text()) for p in (HERE / "probes").glob("*/task.json")
    }


class CategoriesTest(unittest.TestCase):
    def test_every_task_has_exactly_one_known_category(self) -> None:
        for tid, spec in specs().items():
            self.assertIn(spec.get("category"), v2.CATEGORIES, tid)

    def test_v2_tasks_exist_are_gui_only_and_valid(self) -> None:
        all_specs = specs()
        self.assertEqual(v2.validate_tasks(all_specs, list(v2.V2_TASKS)), [])
        for tid in v2.V2_TASKS:
            self.assertFalse(all_specs[tid].get("coding_tools"), tid)
        self.assertEqual(
            {all_specs[t]["category"] for t in v2.V2_TASKS if t.startswith("IR-")}, {"interruptions"}
        )

    def test_coding_task_and_missing_category_are_refused(self) -> None:
        all_specs = specs()
        problems = v2.validate_tasks(all_specs, ["CDB-S01"])
        self.assertTrue(any("coding tools" in p for p in problems), problems)
        bad = {"X": {"id": "X"}}
        self.assertTrue(v2.validate_tasks(bad, ["X"]))
        self.assertTrue(v2.validate_tasks(all_specs, ["NOPE"]))

    def test_background_is_not_a_category(self) -> None:
        self.assertNotIn("background", v2.CATEGORIES)


class ScanTest(unittest.TestCase):
    def test_exec_patterns_are_violations_and_app_names_are_mentions(self) -> None:
        hard, soft = v2.scan_tool_use("mcp__codex-cu__js", {"code": "require('fs').writeFileSync('/x', 'y')"})
        self.assertTrue(hard)
        hard, soft = v2.scan_tool_use("mcp__cua__launch_app", {"app": "Terminal"})
        self.assertEqual(hard, [])
        self.assertEqual(soft, ["mcp__cua__launch_app:Terminal"])
        hard, _ = v2.scan_tool_use("mcp__cua__run_script", {"script": "await app.click({name:'OK'})"})
        self.assertEqual(hard, [])

    def test_node_module_imports_are_violations(self) -> None:
        for code in ("const fs = await import('fs')", "require(`child_process`)", "import ( \"node:os\" )"):
            self.assertTrue(v2.scan_tool_use("mcp__codex-cu__js", {"code": code})[0], code)
        self.assertEqual(v2.scan_tool_use("mcp__codex-cu__js", {"code": "await cua.getApp('Mail')"})[0], [])

    def test_forbidden_builtins_are_violations_and_read_is_not(self) -> None:
        self.assertEqual(v2.scan_tool_use("Bash", {"command": "ls"})[0], ["builtin:Bash"])
        self.assertEqual(v2.scan_tool_use("Read", {"file_path": "/etc/hosts"}), ([], []))

    def test_scan_events_reads_assistant_tool_uses(self) -> None:
        events = [
            {"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "mcp__cua__type_text", "input": {"text": "osascript -e 1"}}]}},
            {"type": "user", "message": {"content": [{"type": "tool_result", "content": "osascript"}]}},
        ]
        hard, soft = v2.scan_events(events)
        self.assertEqual(hard, ["mcp__cua__type_text:osascript"])


class GuardTest(unittest.TestCase):
    def make(self, fronts: list[str | None], procs: set[str] = frozenset(), enforce: bool = True):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        stream = Path(tmp.name) / "claude-stream.tsv"
        stream.write_text("")
        it = iter(fronts)
        guard = v2.GuiOnlyGuard(
            stream, lambda: next(it, None), enforce=enforce, pgrep=lambda name: name in procs
        )
        return guard, stream

    def test_brief_terminal_flicker_is_transient(self) -> None:
        guard, _ = self.make(["com.apple.Terminal", "com.apple.Terminal", "ai.cua.benchlab"])
        for _ in range(3):
            guard.sample()
        self.assertFalse(guard.stop.is_set())
        self.assertEqual(guard.transient, {"front:Terminal"})
        self.assertEqual(guard.violations, [])

    def test_terminal_in_front_for_three_samples_stops_the_trial(self) -> None:
        guard, _ = self.make(["com.apple.Terminal"] * 3)
        for _ in range(3):
            guard.sample()
        self.assertTrue(guard.stop.is_set())
        self.assertEqual(guard.violations[0]["detail"], "front:Terminal")

    def test_scripting_process_is_a_violation(self) -> None:
        guard, _ = self.make([None], procs={"Script Editor"})
        guard.sample()
        self.assertTrue(guard.stop.is_set())

    def test_stream_tool_input_is_read_incrementally(self) -> None:
        guard, stream = self.make([None] * 3)
        line = {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "mcp__codex-cu__js", "input": {"code": "require(\"fs\")"}}]}}
        guard.sample()
        with stream.open("a") as h:
            h.write("1.0\t" + json.dumps(line))  # no newline yet: not read
        guard.sample()
        self.assertFalse(guard.stop.is_set())
        with stream.open("a") as h:
            h.write("\n")
        guard.sample()
        self.assertTrue(guard.stop.is_set())
        summary = guard.summary([], [])
        self.assertTrue(summary["violation"] and summary["stopped_trial"])

    def test_record_only_mode_never_stops(self) -> None:
        guard, _ = self.make(["com.apple.Terminal"] * 4, enforce=False)
        for _ in range(4):
            guard.sample()
        self.assertFalse(guard.stop.is_set())
        s = guard.summary(["mcp__x:osascript"], ["mcp__x:Terminal"])
        self.assertTrue(s["violation"])
        self.assertFalse(s["stopped_trial"])
        self.assertEqual(len(s["violations"]), 2)

    def test_either_event(self) -> None:
        a, b = threading.Event(), threading.Event()
        e = v2.EitherEvent(a, b)
        self.assertFalse(e.is_set())
        b.set()
        self.assertTrue(e.is_set())


class BackgroundTest(unittest.TestCase):
    QUIET = {
        "available": True, "front_changes": 0, "front_changed_to": [], "key_loss": 0, "activations_lost": 0,
        "keystrokes_leaked": 0, "clicks_leaked": 0, "scrolls_leaked": 0, "pointer_moved": False,
        "pointer_max_deviation_px": 0.0, "windows_raised": 0, "raised_by": [],
    }

    def test_quiet_trial_is_clean(self) -> None:
        self.assertTrue(v2.background_score(self.QUIET, True)["clean"])

    def test_each_disturbance_breaks_clean(self) -> None:
        for key, value in (
            ("front_changes", 1), ("key_loss", 1), ("pointer_moved", True),
            ("keystrokes_leaked", 2), ("windows_raised", 1),
        ):
            d = dict(self.QUIET, **{key: value})
            self.assertFalse(v2.background_score(d, True)["clean"], key)

    def test_unmeasured_is_never_clean(self) -> None:
        self.assertFalse(v2.background_score(self.QUIET, False)["clean"])
        self.assertFalse(v2.background_score(dict(self.QUIET, available=False), True)["clean"])

    def test_old_sentinel_without_window_field(self) -> None:
        out = v2.background_score(dict(self.QUIET, windows_raised=None), True)
        self.assertTrue(out["clean"])
        self.assertFalse(out["windows_measured"])


class SelectionTest(unittest.TestCase):
    def ctx(self, *argv: str) -> rb.Ctx:
        args = rb.build_parser().parse_args(["dry-run", *argv])
        return rb.make_ctx(args)

    def test_v2_selects_the_registered_list_in_order(self) -> None:
        ctx = self.ctx("--bench-v2")
        self.assertEqual(ctx.task_ids, list(v2.V2_TASKS))

    def test_v2_refuses_a_coding_task(self) -> None:
        with self.assertRaises(SystemExit):
            self.ctx("--bench-v2", "--tasks", "CDB-S01")

    def test_v1_default_is_unchanged(self) -> None:
        ctx = self.ctx()
        self.assertFalse(any(t.startswith("IR-") for t in ctx.task_ids))

    def test_v2_static_checks_pass_for_the_headline_arms(self) -> None:
        ctx = self.ctx("--bench-v2", "--arms", "cc-cua-driver-script", "cc-codex-cu", "cc-claude-cu-helper")
        checks = rb.v2_static_checks(ctx)
        self.assertTrue(all(status == "pass" for _, status, _ in checks), checks)


class AnalyzeV2Test(unittest.TestCase):
    def row(self, arm: str, task: str, cat: str, passed: bool, steals: int = 0, violation: bool = False, **extra):
        return {
            "final": True, "excluded": False, "bench_version": 2, "arm": arm, "task": task, "category": cat,
            "trial_id": f"{task}-{arm}-{len(extra)}", "passed": passed and not violation, "passed_raw": passed,
            "turns": 10, "cost_usd": 0.5, "agent_wall_s": 100.0,
            "background": v2.background_score(dict(BackgroundTest.QUIET, front_changes=steals), True),
            "gui_only": {"violation": violation, "violations": [{"detail": "front:Terminal"}] if violation else []},
            **extra,
        }

    def test_report_by_category_and_headline(self) -> None:
        sys.path.insert(0, str(HERE / "tools"))
        import analyze_v2 as av  # noqa: E402

        intr = {"kind": "consent_overlay", "shown": True, "exercised": True, "handled": False, "completed": True}
        rows = [
            self.row("cc-cua-driver-script", "CDB-G04", "multi_app", True),
            self.row("cc-cua-driver-script", "MB-10", "precision", True, steals=2),
            self.row("cc-cua-driver-script", "IR-04", "interruptions", False, interruption=intr),
            self.row("cc-codex-cu", "CDB-G04", "multi_app", True, violation=True),
            {"final": True, "bench_version": 1, "arm": "cc-codex-cu", "task": "MB-10"},  # v1 rows are ignored
        ]
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
            report = av.analyze(av.load_rows([Path(tmp)]))
        a = report["arms"]["cc-cua-driver-script"]
        self.assertEqual(a["label"], "Cua Driver")
        self.assertEqual(av.headline(a["overall"]), "2/3 passed, 2 focus steals")
        self.assertEqual(a["by_category"]["precision"]["success"]["k"], 1)
        self.assertIsNone(a["by_category"]["web_forms"])
        self.assertEqual(a["interruptions"]["IR-04"]["handled"]["k"], 0)
        b = report["arms"]["cc-codex-cu"]
        self.assertEqual(b["overall"]["success"]["k"], 0)
        self.assertEqual(b["overall"]["passed_raw"], 1)
        self.assertEqual(len(b["violations"]), 1)
        self.assertIn("## Interruptions", av.markdown(report))


if __name__ == "__main__":
    unittest.main()
