# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The GUI-only guard on Windows (the counterpart of the macOS and Linux rules in bench_v2.py).

A trial fails, and is stopped, when the agent reaches a shell or script host through the GUI:
* a side-door process (PowerShell, cmd, Windows Terminal, the script hosts, mshta, WSL, ...) starts during
  the trial outside the runner's own process tree (the runner, the agent and its tool servers are never
  counted, and processes that were already running when the trial started are ignored), or
* a side-door window is the foreground window for FRONT_GRACE_SAMPLES samples in a row, or
* a computer-use tool input carries code that runs outside the GUI (bench_v2.EXEC_PATTERNS, plus the Windows
  patterns below).

The pure functions here take process rows and window owners as data, so they are tested on every OS.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any, Iterable

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bench_v2 as v2  # noqa: E402

# Executable names (lower case, without .exe) that are a shell, a console or a script host.
SIDE_DOOR_PROCESSES = {
    "powershell": "Windows PowerShell",
    "powershell_ise": "PowerShell ISE",
    "pwsh": "PowerShell",
    "cmd": "Command Prompt",
    "windowsterminal": "Windows Terminal",
    "openconsole": "Windows Terminal",
    "wt": "Windows Terminal",
    "wscript": "Windows Script Host",
    "cscript": "Windows Script Host",
    "mshta": "HTML Application host",
    "wsl": "WSL",
    "wslhost": "WSL",
    "bash": "bash",
}
# Text in a tool input that runs code outside the GUI on Windows.
WINDOWS_EXEC_PATTERNS = (
    "powershell",
    "pwsh",
    "cmd.exe",
    "cmd /c",
    "cmd /k",
    "wscript",
    "cscript",
    "mshta",
    "Start-Process",
    "Invoke-Expression",
    "Invoke-Item",
    "rundll32",
    "wsl.exe",
)
FRONT_GRACE_SAMPLES = v2.FRONT_GRACE_SAMPLES


def name_of(exe: str) -> str:
    base = exe.replace("\\", "/").rsplit("/", 1)[-1].lower()
    return base[:-4] if base.endswith(".exe") else base


def descendants(rows: Iterable[tuple[int, int, str]], root: int) -> set[int]:
    """`root` and every process below it."""
    children: dict[int, list[int]] = {}
    for pid, ppid, _ in rows:
        if pid != ppid:
            children.setdefault(ppid, []).append(pid)
    out, todo = set(), [root]
    while todo:
        pid = todo.pop()
        if pid in out:
            continue
        out.add(pid)
        todo.extend(children.get(pid, []))
    return out


def new_side_doors(
    rows: list[tuple[int, int, str]], baseline: set[int], runner: int, own: set[int] | None = None
) -> list[tuple[int, str]]:
    """Side-door processes that started after the baseline and are not under the runner. `own` keeps every
    pid ever seen under the runner, so a process whose parent has exited still counts as the runner's."""
    own = descendants(rows, runner) | (own if own is not None else set())
    return [
        (pid, SIDE_DOOR_PROCESSES[name_of(exe)])
        for pid, _, exe in rows
        if pid not in baseline and pid not in own and name_of(exe) in SIDE_DOOR_PROCESSES
    ]


def started_by_tool(
    rows: Iterable[tuple[int, int, str]], pid: int, tool_exes: Iterable[str]
) -> str | None:
    """The executable name of `pid`'s parent when that parent is one of the setup's own tool binaries
    (`--tool-exe`), else None. A shell a driver binary starts for itself (to query a process, say) is part of
    the tool, as a helper process is on macOS; a shell started by code the agent supplied (a harness's Python
    `exec`, a JavaScript REPL) has an interpreter as its parent and stays a side door."""
    names = {name_of(t) for t in tool_exes}
    if not names:
        return None
    parent = {p: pp for p, pp, _ in rows}.get(pid)
    exe = {p: e for p, _, e in rows}.get(parent, "") if parent is not None else ""
    return name_of(exe) if name_of(exe) in names else None


def scan_tool_use(name: str, tool_input: Any) -> tuple[list[str], list[str]]:
    hard, soft = v2.scan_tool_use(name, tool_input)
    if name.startswith("mcp__"):
        text = "\n".join(v2._strings(tool_input)).lower()
        hard += [f"{name}:{p}" for p in WINDOWS_EXEC_PATTERNS if p.lower() in text]
    return sorted(set(hard)), soft


def scan_events(events: list[dict[str, Any]]) -> tuple[list[str], list[str]]:
    hard: set[str] = set()
    soft: set[str] = set()
    for event in events:
        for name, tool_input in v2.tool_uses(event):
            h, s = scan_tool_use(name, tool_input)
            hard.update(h)
            soft.update(s)
    return sorted(hard)[:20], sorted(soft)[:20]


class FrontWatch:
    """A side-door window in front for FRONT_GRACE_SAMPLES samples in a row is a violation."""

    def __init__(self) -> None:
        self.run = 0
        self.seen: set[str] = set()

    def sample(self, front_exe: str) -> str | None:
        name = name_of(front_exe)
        if name:
            self.seen.add(name)
        if name in SIDE_DOOR_PROCESSES:
            self.run += 1
            if self.run >= FRONT_GRACE_SAMPLES:
                return SIDE_DOOR_PROCESSES[name]
        else:
            self.run = 0
        return None
