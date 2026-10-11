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

    def test_amendment_18_set_is_valid_and_balanced(self) -> None:
        all_specs = specs()
        self.assertEqual(v2.validate_tasks(all_specs, list(v2.V2_42_TASKS)), [])
        self.assertEqual(len(set(v2.V2_42_TASKS)), len(v2.V2_42_TASKS))
        self.assertEqual(v2.V2_42_TASKS[: len(v2.V2_TASKS)], v2.V2_TASKS)  # the A14.6 order is kept
        for tid in v2.V2_NEW_TASKS:
            spec = all_specs[tid]
            self.assertEqual(spec["kind"], "cdb", tid)
            self.assertTrue(spec["pack_task"].startswith("v2/"), tid)
            self.assertFalse(spec.get("coding_tools"), tid)
        counts: dict[str, int] = {}
        for tid in v2.V2_42_TASKS:
            counts[all_specs[tid]["category"]] = counts.get(all_specs[tid]["category"], 0) + 1
        self.assertEqual(set(counts), set(v2.CATEGORIES))
        self.assertTrue(all(6 <= n <= 8 for n in counts.values()), counts)

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

    def test_terminal_shell_input_in_background_is_a_violation(self) -> None:
        # A19: Terminal is never in front (background typing), but its idle shell reads input.
        ttys = iter([{"/dev/ttys003": 100.0}, {"/dev/ttys003": 100.0}, {"/dev/ttys003": 131.5}])
        guard, _ = self.make([None] * 3)
        guard.terminal_ttys = lambda: next(ttys)
        guard.sample()
        guard.sample()
        self.assertFalse(guard.stop.is_set())
        guard.sample()
        self.assertTrue(guard.stop.is_set())
        self.assertEqual(guard.violations[0]["detail"], "terminal:input")
        self.assertEqual(guard.summary([], [])["terminal_ttys_at_start"], 1)

    def test_new_terminal_session_is_a_violation(self) -> None:
        ttys = iter([{}, {"/dev/ttys009": 5.0}])
        guard, _ = self.make([None] * 2)
        guard.terminal_ttys = lambda: next(ttys)
        guard.sample()
        guard.sample()
        self.assertEqual(guard.violations[0]["detail"], "terminal:new_session")

    def test_closed_terminal_session_is_not_a_violation(self) -> None:
        ttys = iter([{"/dev/ttys003": 1.0}, {}])
        guard, _ = self.make([None] * 2)
        guard.terminal_ttys = lambda: next(ttys)
        guard.sample()
        guard.sample()
        self.assertEqual(guard.violations, [])

    def test_terminal_session_ttys_skip_the_runner_and_non_terminal_ttys(self) -> None:
        rows = [
            (1, 0, "??", "/sbin/launchd"),
            (100, 1, "??", "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"),
            (200, 100, "s000", "login"),
            (201, 200, "s000", "-zsh"),
            (202, 201, "s000", "/usr/bin/python3"),  # the runner, started from launch_bench.command
            (203, 202, "s000", "claude"),
            (204, 203, "s004", "codex"),  # a pty opened under the runner is the runner's own
            (300, 100, "s001", "login"),
            (301, 300, "s001", "-zsh"),  # a stray idle shell window
            (400, 1, "s005", "sshd-session"),  # not a Terminal session
        ]
        self.assertEqual(v2.terminal_session_ttys(rows, exclude_pid=202), {"/dev/ttys001"})
        self.assertEqual(
            v2.terminal_session_ttys(rows, exclude_pid=999), {"/dev/ttys000", "/dev/ttys001", "/dev/ttys004"}
        )

    def test_terminal_tty_inputs_runs_on_this_host(self) -> None:
        self.assertIsInstance(v2.terminal_tty_inputs(), dict)

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
        a = report["arms"]["cc-cua-driver-script|foreground"]
        self.assertEqual(a["label"], "Cua Driver, driven by Claude Code [foreground]")
        self.assertEqual(av.headline(a["overall"]), "2/3 passed, 2 focus steals")
        self.assertEqual(a["by_category"]["precision"]["success"]["k"], 1)
        self.assertIsNone(a["by_category"]["web_forms"])
        self.assertEqual(a["interruptions"]["IR-04"]["handled"]["k"], 0)
        b = report["arms"]["cc-codex-cu|foreground"]
        self.assertEqual(b["overall"]["success"]["k"], 0)
        self.assertEqual(b["overall"]["passed_raw"], 1)
        self.assertEqual(len(b["violations"]), 1)
        self.assertIn("## Interruptions", av.markdown(report))


class BackgroundConditionTest(unittest.TestCase):
    """Amendment 15."""

    def bg(self, **kw):
        return v2.background_score(dict(BackgroundTest.QUIET, **kw), True)

    def test_completion_needs_pass_and_held_window(self) -> None:
        ua = {"typed": 30, "blocked": 0, "text_disruptions": 0, "text_intact_at_end": True, "disrupted": False}
        d = dict(BackgroundTest.QUIET, user_activity=ua, overlay_owners=["Cua Driver Bench Script"])
        out = v2.background_condition_score(self.bg(), d, True, "background")
        self.assertTrue(out["background_completion"])
        self.assertEqual(out["overlay_owners"], ["Cua Driver Bench Script"])
        self.assertFalse(v2.background_condition_score(self.bg(), d, False, "background")["background_completion"])
        steal = v2.background_condition_score(self.bg(front_changes=1), d, True, "background")
        self.assertFalse(steal["background_completion"])
        self.assertTrue(steal["success_regardless_of_focus"])
        typed_into = dict(d, user_activity=dict(ua, text_disruptions=1, disrupted=True))
        out = v2.background_condition_score(self.bg(), typed_into, True, "background")
        self.assertFalse(out["background_completion"])
        self.assertTrue(out["user_input_disrupted"])

    def test_unmeasured_window_order_or_user_text_is_not_completion(self) -> None:
        # Amendment 19: a missing measure is "not held", never a pass.
        ua = {"typed": 30, "blocked": 0, "text_disruptions": 0, "text_intact_at_end": True, "disrupted": False}
        old_sentinel = v2.background_score(dict(BackgroundTest.QUIET, windows_raised=None), True)
        out = v2.background_condition_score(old_sentinel, dict(BackgroundTest.QUIET, user_activity=ua), True, "background")
        self.assertFalse(out["background_completion"])
        no_ua = v2.background_condition_score(self.bg(), BackgroundTest.QUIET, True, "background")
        self.assertFalse(no_ua["background_completion"])
        self.assertTrue(v2.background_condition_score(self.bg(), BackgroundTest.QUIET, True, "foreground")["user_window_held"])

    def test_foreground_condition_has_no_completion_metric(self) -> None:
        out = v2.background_condition_score(self.bg(), BackgroundTest.QUIET, True, "foreground")
        self.assertIsNone(out["background_completion"])

    def test_sentinel_args_and_brief(self) -> None:
        self.assertEqual(v2.sentinel_args("foreground"), [])
        self.assertIn("--user-activity", v2.sentinel_args("background"))
        self.assertIn("User activity", v2.brief_for("Do X.", "background"))
        self.assertEqual(v2.brief_for("Do X.", "foreground"), "Do X.")



class PointerStillTest(unittest.TestCase):
    """A19.5 item 1: background completion with the pointer kept still, reported alongside A15.3."""

    def score(self, pointer_moved: object) -> dict:
        bg = {"measured": True, "focus_steals": 0, "key_focus_lost": 0, "windows_raised": 0, "input_leaked": 0,
              "pointer_moved": pointer_moved}
        dist = {"user_activity": {"text_disruptions": 0, "text_intact_at_end": True}}
        return v2.background_condition_score(bg, dist, True, "background")

    def test_pointer_moved_fails_only_the_pointer_still_variant(self) -> None:
        moved = self.score(True)
        self.assertTrue(moved["background_completion"])
        self.assertFalse(moved["background_completion_pointer_still"])
        self.assertTrue(self.score(False)["background_completion_pointer_still"])
        self.assertFalse(self.score(None)["background_completion_pointer_still"])

    def test_foreground_has_no_variant(self) -> None:
        self.assertIsNone(v2.background_condition_score({}, {}, True, "foreground")["background_completion_pointer_still"])



class MultiplayerTest(unittest.TestCase):
    """Amendment 22: background completion with the user undisturbed (eight criteria)."""

    CLEAN = {
        "key_loss": 0,
        "windows_raised": 0,
        "keystrokes_leaked": 0,
        "clicks_leaked": 0,
        "scrolls_leaked": 0,
        "user_activity": {"blocked": 0, "text_disruptions": 0, "text_intact_at_end": True},
        "pointer_max_deviation_px": 0.0,
        "clipboard_changes": 0,
        "space_switches": 0,
        "sheets_shown": 0,
        "front_changes": 0,
        "activations_lost": 0,
        "activations_gained": 0,
    }

    def test_clean_trial_completes(self) -> None:
        out = v2.multiplayer_score(dict(self.CLEAN), True, True, "background")
        self.assertTrue(out["background_completion"])
        self.assertEqual(out["unmeasured"], [])

    def test_each_disturbance_fails_completion(self) -> None:
        cases = {
            "key_focus": {"key_loss": 1},
            "window_raised": {"windows_raised": 2},
            "typing": {
                "user_activity": {"blocked": 1, "text_disruptions": 0, "text_intact_at_end": True}
            },
            "pointer": {"pointer_max_deviation_px": 3.0},
            "clipboard": {"clipboard_changes": 1},
            "space": {"space_switches": 1},
            "dialog": {"sheets_shown": 1},
            "activation": {"activations_gained": 1},
        }
        self.assertEqual(set(cases), set(v2.MULTIPLAYER_CRITERIA))
        for name, change in cases.items():
            out = v2.multiplayer_score({**self.CLEAN, **change}, True, True, "background")
            self.assertFalse(out["background_completion"], name)
            self.assertIs(out["criteria"][name], False, name)

    def test_unmeasured_fails_closed_unless_rescoring(self) -> None:
        old = {
            k: v
            for k, v in self.CLEAN.items()
            if k not in ("clipboard_changes", "space_switches", "sheets_shown")
        }
        self.assertFalse(
            v2.multiplayer_score(old, True, True, "background")["background_completion"]
        )
        rescored = v2.multiplayer_score(old, True, True, "background", rescore=True)
        self.assertTrue(rescored["background_completion"])
        self.assertEqual(rescored["unmeasured"], ["clipboard", "dialog", "space"])
        self.assertEqual(rescored["rule"], "A22 rescored")

    def test_typed_input_landing_in_the_user_window_is_typing_disturbed(self) -> None:
        out = v2.multiplayer_score({**self.CLEAN, "keystrokes_leaked": 4}, True, True, "background")
        self.assertIs(out["criteria"]["typing"], False)

if __name__ == "__main__":
    unittest.main()
