# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The GUI-only guard on Hyprland (the counterpart of windows/guard.py and the X11 rules in bench_v2.py).

A trial fails, and is stopped, when the agent reaches a shell through the GUI:
* a terminal emulator starts during the trial outside the runner's own process tree (processes already running
  when the trial started are ignored), or
* a terminal window is the active window for FRONT_GRACE_SAMPLES samples in a row, or
* a computer-use tool input carries code that runs outside the GUI (bench_v2.EXEC_PATTERNS, plus the Hyprland
  patterns below: dispatching `exec` to the compositor, Omarchy's and uwsm's launchers, terminal names).

Shells alone are not counted as processes: the tool servers reach the desktop through ssh, which runs them under a
shell. A shell the agent could type into needs a terminal window, which is caught.

The pure functions take process rows and window classes as data, so they are tested on every OS.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path
from typing import Any, Iterable

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bench_v2 as v2  # noqa: E402

# Process names (comm, lower case) of terminal emulators.
TERMINALS = {
    "alacritty": "Alacritty",
    "kitty": "kitty",
    "foot": "foot",
    "footclient": "foot",
    "ghostty": "Ghostty",
    "wezterm": "WezTerm",
    "wezterm-gui": "WezTerm",
    "xterm": "xterm",
    "uxterm": "xterm",
    "urxvt": "urxvt",
    "st": "st",
    "konsole": "Konsole",
    "gnome-terminal": "GNOME Terminal",
    "gnome-terminal-": "GNOME Terminal",
    "kgx": "GNOME Console",
    "ptyxis": "Ptyxis",
    "ptyxis-agent": "Ptyxis",
    "xfce4-terminal": "Xfce Terminal",
    "terminator": "Terminator",
    "tilix": "Tilix",
    "rio": "Rio",
    "warp": "Warp",
}
# Window classes (lower case) of terminal windows, Omarchy's floating and tiled TUI classes included.
TERMINAL_CLASSES = {
    "alacritty": "Alacritty",
    "kitty": "kitty",
    "foot": "foot",
    "com.mitchellh.ghostty": "Ghostty",
    "ghostty": "Ghostty",
    "org.wezfurlong.wezterm": "WezTerm",
    "xterm": "xterm",
    "urxvt": "urxvt",
    "st-256color": "st",
    "org.kde.konsole": "Konsole",
    "org.gnome.terminal": "GNOME Terminal",
    "org.gnome.console": "GNOME Console",
    "org.gnome.ptyxis": "Ptyxis",
    "xfce4-terminal": "Xfce Terminal",
    "tui.float": "Omarchy TUI",
    "tui.tile": "Omarchy TUI",
}
# Text in a tool input that runs code outside the GUI on Hyprland/Omarchy.
HYPRLAND_EXEC_PATTERNS = (
    "hyprctl",
    "dispatch exec",
    "uwsm app",
    "uwsm-app",
    "omarchy-launch",
    "xdg-terminal-exec",
    "systemd-run",
    "sh -c",
)
# Terminal names in a tool input (launching one opens a shell): whole words only.
TERMINAL_NAME_RE = re.compile(
    r"\b(alacritty|ghostty|kitty|foot|footclient|wezterm|xterm|konsole|gnome-terminal|kgx|ptyxis)\b"
)
FRONT_GRACE_SAMPLES = v2.FRONT_GRACE_SAMPLES


def terminal_of(name: str) -> str | None:
    return TERMINALS.get(name.lower())


def process_rows() -> list[tuple[int, int, str]]:
    """(pid, ppid, comm) for every process."""
    rows = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            stat = Path("/proc", entry, "stat").read_text()
        except OSError:
            continue
        name = stat[stat.index("(") + 1 : stat.rindex(")")]
        ppid = int(stat[stat.rindex(")") + 2 :].split()[1])
        rows.append((int(entry), ppid, name))
    return rows


def descendants(rows: Iterable[tuple[int, int, str]], root: int) -> set[int]:
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
    own = descendants(rows, runner) | (own if own is not None else set())
    return [
        (pid, terminal_of(name))
        for pid, _, name in rows
        if pid not in baseline and pid not in own and terminal_of(name)
    ]


def scan_tool_use(name: str, tool_input: Any) -> tuple[list[str], list[str]]:
    hard, soft = v2.scan_tool_use(name, tool_input)
    if name.startswith("mcp__"):
        text = "\n".join(v2._strings(tool_input)).lower()
        hard += [f"{name}:{p}" for p in HYPRLAND_EXEC_PATTERNS if p in text]
        hard += [f"{name}:{m}" for m in TERMINAL_NAME_RE.findall(text)]
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
    """A terminal window active for FRONT_GRACE_SAMPLES samples in a row is a violation."""

    def __init__(self) -> None:
        self.run = 0
        self.seen: set[str] = set()

    def sample(self, window_class: str) -> str | None:
        name = (window_class or "").lower()
        if name:
            self.seen.add(name)
        label = TERMINAL_CLASSES.get(name) or (
            "Omarchy TUI" if name.startswith("org.omarchy.") else None
        )
        if label:
            self.run += 1
            if self.run >= FRONT_GRACE_SAMPLES:
                return label
        else:
            self.run = 0
        return None
