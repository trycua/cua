"""Bench v2 (Amendment 14): GUI-only enforcement, task categories, the background score and the
interruption fields of a trial row.

* GUI-only (CUA-1200). Every v2 task runs once, with no shell or file tools for any setup. The
  surface is the same for every setup (Skill, a skill-scoped Read, ToolSearch and the setup's own
  computer-use server). `GuiOnlyGuard` watches the agent phase of every trial for the routes that
  are left: a terminal or scripting app in front, a scripting process, or a computer-use tool input
  that runs code or touches files outside the GUI. A violation ends the trial at once and the trial
  counts as failed (`passed` false, `passed_raw` keeps the evaluator's verdict).
* Categories (CUA-1281). Every task has exactly one `category` in its task.json. Reports aggregate
  per category and overall. Background operation is a score on every trial, not a category.
* Background score (CUA-1282). From the BenchSentinel witness, which is frontmost and key before
  every v2 trial: focus steals, key-focus loss, real-pointer movement, windows raised above it and
  input that landed in it. A trial is background-clean when all of these are zero.
* Interruptions (CUA-1283). The IR-* evaluators write an `interruption` diagnostic (shown,
  exercised, handled, completed); the row carries it.
"""

from __future__ import annotations

import json
import re
import subprocess
import threading
import time
from pathlib import Path
from typing import Any, Callable

BENCH_VERSION = 2

CATEGORIES: dict[str, str] = {  # report order
    "multi_app": "Multi-app workflows",
    "web_forms": "Web apps & forms",
    "precision": "Precision interactions",
    "interruptions": "Interruptions & recovery",
}
BACKGROUND_LABEL = "Background operation"  # scored on every trial; never a task bucket

# The v2 task list, in schedule order. Each exists once and is GUI-only.
V2_TASKS: tuple[str, ...] = (
    "CDB-G02",
    "CDB-G03",
    "CDB-G04",
    "MB-09",
    "MB-10",
    "MB-11",
    "IR-01",
    "IR-02",
    "IR-03",
    "IR-04",
)
# v1 tasks that are not in v2, and why (the S/G split is dropped).
NOT_IN_V2: dict[str, str] = {
    "CDB-S01": "its end state needs a source-code fix and a test; no GUI-only version without changing the pack",
    "CDB-S02": "replaced by its GUI-only version CDB-G02",
    "CDB-S03": "replaced by its GUI-only version CDB-G03",
    "CDB-S04": "replaced by its GUI-only version CDB-G04",
}

# ------------------------------------------------------------------ categories


def category_of(spec: dict[str, Any]) -> str:
    category = spec.get("category")
    if category not in CATEGORIES:
        raise ValueError(f"task {spec.get('id')!r} has category {category!r}; known {sorted(CATEGORIES)}")
    return str(category)


def validate_tasks(specs: dict[str, dict[str, Any]], task_ids: list[str]) -> list[str]:
    """Problems that stop a v2 run: unknown task, missing or unknown category, coding tools."""
    problems: list[str] = []
    for tid in task_ids:
        spec = specs.get(tid)
        if spec is None:
            problems.append(f"{tid}: unknown task")
            continue
        try:
            category_of(spec)
        except ValueError as error:
            problems.append(str(error))
        if spec.get("coding_tools"):
            problems.append(f"{tid}: has coding tools; v2 is GUI-only ({NOT_IN_V2.get(tid, 'not a v2 task')})")
        if spec.get("status") == "dropped":
            problems.append(f"{tid}: dropped")
    return problems


# ------------------------------------------------------------------ GUI-only

SIDE_DOOR_BUNDLES = {
    "com.apple.Terminal": "Terminal",
    "com.googlecode.iterm2": "iTerm",
    "com.apple.ScriptEditor2": "Script Editor",
    "com.apple.automator": "Automator",
    "com.apple.shortcuts": "Shortcuts",
}
SIDE_DOOR_PROCESSES = ("Script Editor", "Automator", "iTerm2", "Shortcuts")
# Tool-input text that runs code outside the GUI or reads/writes files (a violation).
EXEC_PATTERNS = (
    "osascript",
    "do shell script",
    "child_process",
    "execSync",
    "spawnSync",
    "execFile",
    "/bin/sh",
    "/bin/zsh",
    "/bin/bash",
    "bash -c",
    "zsh -c",
    "subprocess",
    "os.system",
    'require("fs")',
    "require('fs')",
    "node:fs",
    "node:child_process",
    "readFileSync",
    "writeFileSync",
    "fs.promises",
    "process.binding",
    "NSTask",
)
# A require() or import() of a Node module that reaches files, processes or the network.
NODE_MODULE_RE = re.compile(
    r"""(?:require|import)\s*\(\s*['"`](?:node:)?"""
    r"""(fs|fs/promises|child_process|os|net|http|https|worker_threads|vm|module)['"`]\s*\)"""
)
# Text that only names a side-door app (recorded, not a violation; opening one is caught live).
MENTION_PATTERNS = ("Terminal", "iTerm", "Script Editor", "ScriptEditor", "Automator", "Shortcuts")
# Built-in tools that would be a shell or file route; v2 argv never offers them.
FORBIDDEN_BUILTINS = ("Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch", "Task", "Agent")
FRONT_GRACE_SAMPLES = 3  # a side-door app in front for 3 samples in a row (about 1.5 s) is a violation


def scan_tool_use(name: str, tool_input: Any) -> tuple[list[str], list[str]]:
    """(violations, mentions) for one tool call."""
    if name in FORBIDDEN_BUILTINS:
        return [f"builtin:{name}"], []
    if not name.startswith("mcp__"):
        return [], []
    text = "\n".join(_strings(tool_input))
    hard = [f"{name}:{p}" for p in EXEC_PATTERNS if p in text]
    hard += [f"{name}:import:{m}" for m in sorted(set(NODE_MODULE_RE.findall(text)))]
    soft = [f"{name}:{p}" for p in MENTION_PATTERNS if p in text]
    return hard, soft


def _strings(node: Any) -> list[str]:
    """Every string in a tool input (keys too), unescaped, so quoted code matches the patterns."""
    if isinstance(node, str):
        return [node]
    if isinstance(node, dict):
        return [s for k, v in node.items() for s in (str(k), *_strings(v))]
    if isinstance(node, list):
        return [s for v in node for s in _strings(v)]
    return []


def tool_uses(event: dict[str, Any]) -> list[tuple[str, Any]]:
    if event.get("type") != "assistant":
        return []
    out = []
    for block in (event.get("message") or {}).get("content") or []:
        if isinstance(block, dict) and block.get("type") == "tool_use":
            out.append((str(block.get("name")), block.get("input", {})))
    return out


def scan_events(events: list[dict[str, Any]]) -> tuple[list[str], list[str]]:
    hard: set[str] = set()
    soft: set[str] = set()
    for event in events:
        for name, tool_input in tool_uses(event):
            h, s = scan_tool_use(name, tool_input)
            hard.update(h)
            soft.update(s)
    return sorted(hard)[:20], sorted(soft)[:20]


class EitherEvent:
    """is_set() of either event: the run's abort or this trial's GUI-only stop (run_claude polls it)."""

    def __init__(self, *events: threading.Event) -> None:
        self.events = events

    def is_set(self) -> bool:
        return any(e.is_set() for e in self.events)


def _pgrep(name: str) -> bool:
    return subprocess.run(["pgrep", "-x", name], capture_output=True).returncode == 0


class GuiOnlyGuard(threading.Thread):
    """Samples the frontmost app and the scripting processes every `interval` seconds and reads the
    agent's stream as it grows. The first violation sets `stop` (when enforcing), which ends the trial."""

    def __init__(
        self,
        stream_path: Path,
        frontmost: Callable[[], str | None],
        enforce: bool = True,
        interval: float = 0.5,
        pgrep: Callable[[str], bool] = _pgrep,
    ) -> None:
        super().__init__(daemon=True)
        self.stream_path = stream_path
        self.frontmost = frontmost
        self.enforce = enforce
        self.interval = interval
        self.pgrep = pgrep
        self.stop = threading.Event()
        self.done = threading.Event()
        self.started_mono = time.monotonic()
        self.seen: set[str] = set()
        self.violations: list[dict[str, Any]] = []
        self.transient: set[str] = set()
        self.mentions: set[str] = set()
        self._run_of: dict[str, int] = {}
        self._offset = 0

    def _violate(self, kind: str, detail: str) -> None:
        if any(v["detail"] == detail for v in self.violations):
            return
        self.violations.append(
            {"t_s": round(time.monotonic() - self.started_mono, 2), "kind": kind, "detail": detail}
        )
        if self.enforce:
            self.stop.set()

    def sample(self) -> None:
        bundle = self.frontmost()
        if bundle:
            self.seen.add(bundle)
        for bid, label in SIDE_DOOR_BUNDLES.items():
            if bundle == bid:
                self._run_of[bid] = self._run_of.get(bid, 0) + 1
                if self._run_of[bid] >= FRONT_GRACE_SAMPLES:
                    self._violate("front", f"front:{label}")
                else:
                    self.transient.add(f"front:{label}")
            else:
                self._run_of[bid] = 0
        for name in SIDE_DOOR_PROCESSES:
            if self.pgrep(name):
                self._violate("process", f"process:{name}")
        self.read_stream()

    def read_stream(self) -> None:
        try:
            with self.stream_path.open("rb") as handle:
                handle.seek(self._offset)
                chunk = handle.read()
        except OSError:
            return
        end = chunk.rfind(b"\n")
        if end < 0:
            return
        self._offset += end + 1
        for line in chunk[:end].decode("utf-8", "replace").split("\n"):
            _, _, raw = line.partition("\t")
            try:
                event = json.loads(raw)
            except json.JSONDecodeError:
                continue
            for name, tool_input in tool_uses(event):
                hard, soft = scan_tool_use(name, tool_input)
                self.mentions.update(soft)
                for item in hard:
                    self._violate("tool_input", item)

    def run(self) -> None:
        while not self.done.is_set():
            try:
                self.sample()
            except Exception:  # noqa: BLE001 - the guard must never take the trial down
                pass
            self.done.wait(self.interval)

    def finish(self) -> None:
        self.done.set()
        self.join(timeout=5)
        try:
            self.read_stream()
        except Exception:  # noqa: BLE001
            pass

    def summary(self, post_hoc_hard: list[str], post_hoc_soft: list[str]) -> dict[str, Any]:
        for item in post_hoc_hard:
            if not any(v["detail"] == item for v in self.violations):
                self.violations.append({"t_s": None, "kind": "tool_input", "detail": item})
        return {
            "enforced": self.enforce,
            "violation": bool(self.violations),
            "violations": self.violations[:20],
            "stopped_trial": self.enforce and self.stop.is_set(),
            "transient_front": sorted(self.transient),
            "mentions": sorted(self.mentions | set(post_hoc_soft))[:20],
            "frontmost_seen": sorted(self.seen),
        }


# ------------------------------------------------------------------ background score


def background_score(disturbance: dict[str, Any], sentinel_frontmost: bool) -> dict[str, Any]:
    """The per-trial background-operation score (Amendment 14, A14.4)."""
    measured = bool(disturbance.get("available")) and sentinel_frontmost
    raised = disturbance.get("windows_raised")
    leaked = int(disturbance.get("keystrokes_leaked") or 0) + int(disturbance.get("clicks_leaked") or 0) + int(
        disturbance.get("scrolls_leaked") or 0
    )
    out = {
        "measured": measured,
        "focus_steals": int(disturbance.get("front_changes") or 0),
        "focus_stolen_by": list(disturbance.get("front_changed_to") or []),
        "key_focus_lost": int(disturbance.get("key_loss") or 0),
        "foreground_activations": int(disturbance.get("activations_lost") or 0),
        "pointer_moved": bool(disturbance.get("pointer_moved")),
        "pointer_max_deviation_px": disturbance.get("pointer_max_deviation_px"),
        "windows_raised": raised,
        "windows_raised_by": list(disturbance.get("raised_by") or []),
        "windows_measured": raised is not None,
        "input_leaked": leaked,
    }
    out["clean"] = (
        measured
        and out["focus_steals"] == 0
        and out["key_focus_lost"] == 0
        and not out["pointer_moved"]
        and leaked == 0
        and (raised or 0) == 0
    )
    return out


# ------------------------------------------------------------------ interruptions


def interruption_of(evaluation: dict[str, Any]) -> dict[str, Any] | None:
    diag = (evaluation.get("diagnostics") or {}).get("interruption")
    value = diag.get("value") if isinstance(diag, dict) else None
    return value if isinstance(value, dict) else None
