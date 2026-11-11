# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Static background-use lint for raw-linux scripts (the `pi-raw-linux` bench arm).

A script is Python source that drives Linux low-level APIs (AT-SPI through gi, Xlib/XTest through ctypes handles,
xdotool) through the `raw` runtime. Before it runs, `lint(source)` walks its AST and returns findings. Each finding
names a rule, the line, what the rule protects, the source of the rule in Cua Driver's own contract, and a
background-safe alternative.

Severity:
* ``block``: the script does not run. Used for pointer, focus and keyboard theft (the background contract), grabs,
  and the bench's GUI-only side doors.
* ``escalatable``: blocked unless the script declares ``raw.escalate_foreground("<reason>")``. The driver's own
  foreground route is an explicit, authorized escalation (``delivery_mode:"foreground"``), never automatic; the
  declaration is its equivalent here and is reported in every result.
* ``warn``: the script runs; the finding is returned with the result.

The lint cannot prove a script safe (an app may move focus by itself, a value may be computed at run time), so the
runtime in ``raw.py`` checks the same rules when a guarded function is looked up, and its hook samples pointer, focus,
stacking and open popups during the run.

Threat model: an honest model that does not know the background contract, or forgets it. The checker tells it, and it
counts what was blocked and what slipped through. It is not a security boundary against a hostile script. The raw
X11 functions are ctypes calls that take pointers, so a script set on escaping could corrupt memory through them. The
bench runs in a disposable session (bwrap, a private /tmp), and the GUI-only guard still watches for side doors.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass

DRV = "libs/cua-driver/rust"
LINUX_MD = f"{DRV}/Skills/cua-driver/LINUX.md"
INPUT_RS = f"{DRV}/crates/platform-linux/src/input/mod.rs"
CONTRACT = "libs/cua-driver/docs/action-result-contract.md"


@dataclass(frozen=True)
class Rule:
    id: str
    title: str
    severity: str  # block | escalatable | warn
    why: str
    source: str
    instead: str


RULES: dict[str, Rule] = {
    r.id: r
    for r in (
        Rule(
            "BG1",
            "real pointer motion or button",
            "escalatable",
            "XTest, XWarpPointer, xdotool mousemove/click and Atspi.generate_mouse_event move or press the user's "
            "real (core) pointer. Background input must not move it; only an explicit desktop/foreground request does.",
            f"{INPUT_RS}:4-15 (no XTest fallback: it delivers to the focused window); "
            f"{DRV}/crates/platform-linux/src/tools/move_cursor.rs:45 (only scope=desktop moves the real pointer); "
            f"{LINUX_MD}:26-31 (foreground is an authorized escalation, never automatic)",
            "Use the element's AT-SPI Action (Atspi.Action.do_action(acc, i)), EditableText or Value interface. "
            "If the task needs a real hover or drag and no accessible route exists, call "
            'raw.escalate_foreground("<why the background route is unavailable>") first.',
        ),
        Rule(
            "BG2",
            "focus, raise or activate",
            "escalatable",
            "XSetInputFocus, XRaiseWindow/XMapRaised, a _NET_ACTIVE_WINDOW or _NET_RESTACK_WINDOW request, wmctrl -a "
            "and xdotool windowactivate/windowfocus/windowraise take focus from the user's window or raise over it.",
            f"{LINUX_MD}:19-20 (background input must not activate or raise the target); "
            f"{DRV}/crates/platform-linux/src/input/focus_guard.rs:4-12 (core focus, active window and top of "
            "stacking must be left as found)",
            "Address the element directly through AT-SPI (do_action, set_text_contents, set_current_value); these "
            "need no focus. Activate only after raw.escalate_foreground(...) when no background route exists.",
        ),
        Rule(
            "BG3",
            "keys to whatever is focused",
            "escalatable",
            "XTestFakeKeyEvent, Atspi.generate_keyboard_event and xdotool key/type/keydown/keyup without --window "
            "deliver to the focused window, which is the user's, not the target.",
            f"{INPUT_RS}:4-15; {LINUX_MD}:29-31 (if activation cannot be proven, refuse instead of sending keys to "
            "whatever is focused)",
            "Set text with the element's EditableText (set_text_contents / insert_text) or Value interface, or press "
            "the button that the key would trigger through its AT-SPI Action.",
        ),
        Rule(
            "BG4",
            "global grab",
            "block",
            "XGrabKeyboard, XGrabPointer, XGrabKey, XGrabButton and XGrabServer take every input device or the "
            "whole server away from the user until released.",
            f"{INPUT_RS}:1797-1803 (the driver dropped its XI2 shield grab: it swallowed input and left the device "
            f"frozen); {DRV}/crates/platform-linux/src/input/focus_guard.rs:24-27 (a held grab cannot be undone)",
            "No grab is needed: AT-SPI actions and EditableText reach the element without one.",
        ),
        Rule(
            "BG5",
            "Wayland or portal input to the compositor focus",
            "escalatable",
            "libei/RemoteDesktop portal input, ydotool and wtype inject into the compositor's focus; there is no "
            "per-window background target on Wayland.",
            f"{DRV}/crates/platform-linux/src/input/delivery.rs:15-23",
            "Use AT-SPI; on Wayland a pixel or key route is foreground only.",
        ),
        Rule(
            "BG6",
            "synthetic XSendEvent to a GTK3/4 or Chromium window",
            "warn",
            "These toolkits drop synthetic events, so the event is dispatched and never arrives.",
            f"{DRV}/crates/platform-linux/src/input/interactive.rs:14-18; "
            f"{DRV}/crates/platform-linux/src/tools/click.rs:1716-1724",
            "Prefer the AT-SPI Action/EditableText route and read the result back.",
        ),
        Rule(
            "BG7",
            "AT-SPI grab_focus",
            "warn",
            "Component.grab_focus moves keyboard focus inside the app; some toolkits also activate the toplevel.",
            f"{DRV}/crates/platform-linux/src/input/focus_guard.rs:4-12; "
            f"{DRV}/crates/platform-linux/src/tools/impl_.rs:2019-2023 (the driver runs GrabFocus under a focus "
            "guard)",
            "Not needed for set_text_contents or do_action. The runtime reports any focus change it causes.",
        ),
        Rule(
            "BG8",
            "clipboard write",
            "warn",
            "Owning the CLIPBOARD selection replaces what the user copied.",
            f"{LINUX_MD}:276 (the driver offers clipboard_write only as an explicit fallback for text)",
            "Set text with EditableText instead.",
        ),
        Rule(
            "GUI1",
            "code or files outside the GUI",
            "block",
            "Bench v2 is GUI-only: no shell, process, file, network or library loading from a computer-use script.",
            "libs/cua-driver-bench/automated-eval/macos_bench/bench_v2.py:164-187 (EXEC_PATTERNS, the GUI-only "
            "guard)",
            "Use only the raw runtime: raw.Atspi, raw.X11/raw.Xtst handles, raw.xdotool(...), raw.windows(), "
            "raw.screenshot().",
        ),
        Rule(
            "GUI2",
            "dynamic code the lint cannot read",
            "block",
            "eval/exec/compile, __import__/importlib, getattr with a computed name, globals()/vars() hide the call "
            "from this check.",
            "this lint (its rules are only enforceable on code it can read)",
            "Call the API by its plain name.",
        ),
        Rule(
            "BG9",
            "chained xdotool commands",
            "escalatable",
            "xdotool runs every command in a chain; a later key or pointer command does not get the first "
            "command's --window and goes to the focused window or the real pointer.",
            f"{INPUT_RS}:4-15 (keys go to the addressed window, never to whatever is focused)",
            "Make one raw.xdotool call per command. Chains are allowed after raw.escalate_foreground(...).",
        ),
        Rule(
            "VR1",
            "action without a read-back",
            "warn",
            "An action that the API accepted is not a confirmed effect.",
            f"{CONTRACT}:149-152 (native API acceptance alone does not justify confirmed)",
            "After the last action, read the element back (get_text, get_current_value, get_state_set, get_name) "
            "or take raw.screenshot() and return it.",
        ),
    )
}

# Function and attribute names, matched on the final name anywhere in the script (a call, an alias or a
# getattr with a constant). Exact names: no fuzzy matching.
NAMES: dict[str, str] = {
    **dict.fromkeys(
        (
            "XTestFakeMotionEvent",
            "XTestFakeRelativeMotionEvent",
            "XTestFakeButtonEvent",
            "XWarpPointer",
            "generate_mouse_event",
            "XTestFakeDeviceMotionEvent",
            "XTestFakeDeviceButtonEvent",
        ),
        "BG1",
    ),
    **dict.fromkeys(
        (
            "XSetInputFocus",
            "XRaiseWindow",
            "XMapRaised",
            "XRestackWindows",
            "XCirculateSubwindowsUp",
            "XSetICFocus",
        ),
        "BG2",
    ),
    **dict.fromkeys(
        ("XTestFakeKeyEvent", "generate_keyboard_event", "XTestFakeDeviceKeyEvent"), "BG3"
    ),
    **dict.fromkeys(
        (
            "XGrabKeyboard",
            "XGrabPointer",
            "XGrabKey",
            "XGrabButton",
            "XGrabServer",
            "XIGrabDevice",
            "XIGrabKeycode",
            "XIGrabButton",
            "XGrabDevice",
        ),
        "BG4",
    ),
    "XSendEvent": "BG6",
    "grab_focus": "BG7",
    **dict.fromkeys(("XSetSelectionOwner", "set_text" + "_clipboard"), "BG8"),
    **dict.fromkeys(("CDLL", "cdll", "PyDLL", "pydll", "LoadLibrary", "dlopen"), "GUI1"),
    **dict.fromkeys(("system", "popen", "spawnv", "execv", "execvp", "fork"), "GUI1"),
}
# Text that names a forbidden request wherever it appears in a string (an atom, a command line).
STRINGS: dict[str, str] = {
    "_NET_ACTIVE_WINDOW": "BG2",
    "_NET_RESTACK_WINDOW": "BG2",
    "wmctrl": "BG2",
    "windowactivate": "BG2",
    "windowfocus": "BG2",
    "windowraise": "BG2",
    "org.freedesktop.portal.RemoteDesktop": "BG5",
    "ydotool": "BG5",
    "wtype": "BG5",
    "libei": "BG5",
}
# Every xdotool subcommand. raw.xdotool runs one command per call: a second subcommand in the arguments (xdotool's
# command chaining) is refused, so each command's own --window decides its delivery.
XDOTOOL_COMMANDS = set(
    "click getactivewindow getmouselocation getwindowfocus getwindowname getwindowpid getwindowgeometry key keydown "
    "keyup mousedown mousemove mousemove_relative mouseup search selectwindow set_desktop set_desktop_for_window "
    "set_num_desktops set_window type windowactivate windowclose windowfocus windowkill windowlower windowmap "
    "windowminimize windowmove windowquit windowraise windowreparent windowsize windowstate windowunmap "
    "behave behave_screen_edge exec script sleep get_desktop get_desktop_for_window get_desktop_viewport "
    "set_desktop_viewport get_num_desktops getwindowclassname".split()
)
# xdotool subcommands (first argument of raw.xdotool) and their rule; anything not listed is allowed.
XDOTOOL: dict[str, str] = {
    **dict.fromkeys(
        ("mousemove", "mousemove_relative", "click", "mousedown", "mouseup"),
        "BG1",
    ),
    **dict.fromkeys(("windowactivate", "windowfocus", "windowraise", "windowmap"), "BG2"),
    **dict.fromkeys(("key", "keydown", "keyup", "type"), "BG3"),  # unless --window (XSendEvent)
    **dict.fromkeys(("exec", "behave", "behave_screen_edge", "script"), "GUI1"),  # runs commands
}
# Modules a script may import. Everything else is GUI1.
IMPORT_OK = {
    "time",
    "json",
    "re",
    "math",
    "itertools",
    "collections",
    "functools",
    "string",
    "textwrap",
    "dataclasses",
    "typing",
    "enum",
    "raw",
}
# Names that reach past the runtime: other GObject modules (GLib/Gio spawn processes, Gdk warps the pointer), the
# CPython C API through ctypes, and the gi loader itself. Scripts get Atspi as a global and need none of these.
ESCAPES = {
    "gi",
    "repository",
    "GLib",
    "Gio",
    "Gdk",
    "GObject",
    "Gtk",
    "pythonapi",
    "PYFUNCTYPE",
    "CFUNCTYPE",
    "cast",
    "memmove",
    "memset",
    "addressof",
    "LibraryLoader",
    "util",
    "from_address",
    "from_buffer",
    "from_buffer_copy",
    "from_param",
    "in_dll",
}
DYNAMIC = {
    "eval",
    "exec",
    "compile",
    "__import__",
    "globals",
    "vars",
    "locals",
    "breakpoint",
    "importlib",
}
FILE_CALLS = {"open"}
ACTIONS = {
    "do_action",
    "set_text_contents",
    "insert_text",
    "delete_text",
    "paste_text",
    "set_current_value",
    "select_child",
    "XTestFakeButtonEvent",
    "XTestFakeKeyEvent",
    "generate_mouse_event",
    "generate_keyboard_event",
    "XSendEvent",
}
READS = {
    "get_text",
    "get_current_value",
    "get_state_set",
    "get_name",
    "get_description",
    "screenshot",
    "dump",
    "find",
    "get_value",
    "contains",
    "windows",
    "get_child_count",
    "get_child_at_index",
    "get_text_contents",
    "describe",
    "pointer",
    "active_window",
}


@dataclass(frozen=True)
class Finding:
    rule: str
    line: int
    detail: str

    @property
    def severity(self) -> str:
        return RULES[self.rule].severity

    def render(self, escalated: bool) -> str:
        r = RULES[self.rule]
        tag = {
            "block": "BLOCKED",
            "escalatable": "FOREGROUND (declared)" if escalated else "BLOCKED",
            "warn": "WARNING",
        }[r.severity]
        return (
            f"{tag} [{r.id} {r.title}] line {self.line}: {self.detail}\n"
            f"  rule: {r.why}\n  source: {r.source}\n  instead: {r.instead}"
        )


@dataclass
class Report:
    findings: list[Finding]
    escalated: bool
    escalation_reason: str | None

    @property
    def blocked(self) -> list[Finding]:
        return [
            f
            for f in self.findings
            if f.severity == "block" or (f.severity == "escalatable" and not self.escalated)
        ]

    @property
    def ok(self) -> bool:
        return not self.blocked

    def render(self) -> str:
        if not self.findings:
            return ""
        head = (
            "Background-use lint: the script was NOT run."
            if not self.ok
            else "Background-use lint: the script ran with these findings."
        )
        if self.escalated:
            head += f' Foreground escalation declared: "{self.escalation_reason}".'
        body = "\n".join(
            f.render(self.escalated) for f in sorted(self.findings, key=lambda f: f.line)
        )
        return f"{head}\n{body}"


def _dotted(node: ast.AST) -> str:
    parts = []
    while isinstance(node, ast.Attribute):
        parts.append(node.attr)
        node = node.value
    if isinstance(node, ast.Name):
        parts.append(node.id)
    return ".".join(reversed(parts))


def _const_str(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.JoinedStr):
        return "".join(v.value for v in node.values if isinstance(v, ast.Constant))
    return None


def xdotool_commands(args: list[str | None]) -> list[tuple[str, bool]]:
    """(command, has its own --window) for each command of an xdotool chain.

    Any argument equal to a command name starts a new command, even when xdotool would read it as data (typing the
    word "exec"). The split errs toward refusing: xdotool's own parse depends on each command's option table, and a
    refusal with its reason is better than a missed command. Type such text with EditableText instead."""
    starts = [i for i, a in enumerate(args) if i == 0 or a in XDOTOOL_COMMANDS]
    out = []
    for n, i in enumerate(starts):
        end = starts[n + 1] if n + 1 < len(starts) else len(args)
        out.append((str(args[i]), "--window" in args[i + 1 : end]))
    return out


def _pos(node: ast.AST) -> tuple[int, int]:
    return getattr(node, "lineno", 0), getattr(node, "col_offset", 0)


class _Walker(ast.NodeVisitor):
    def __init__(self) -> None:
        self.findings: list[Finding] = []
        self.escalation: str | None = None
        self.escalated = False
        self.last_action: tuple[int, int] = (0, 0)
        self.last_read: tuple[int, int] = (0, 0)
        self.seen: set[tuple[str, int]] = set()

    def add(self, rule: str, node: ast.AST, detail: str) -> None:
        line = getattr(node, "lineno", 0)
        if (rule, line) in self.seen:
            return
        self.seen.add((rule, line))
        self.findings.append(Finding(rule, line, detail))

    def name_hit(self, name: str, node: ast.AST) -> None:
        rule = NAMES.get(name)
        if rule == "GUI1" and name in {"system", "popen", "fork", "execv", "execvp", "spawnv"}:
            # only as an attribute (os.system), not a plain local called "system"
            if not isinstance(node, ast.Attribute):
                return
        if rule:
            self.add(rule, node, f"{name}")
        if name in ACTIONS:
            self.last_action = max(self.last_action, _pos(node))
        if name in READS:
            self.last_read = max(self.last_read, _pos(node))

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            if alias.name not in IMPORT_OK:
                self.add("GUI1", node, f"import {alias.name}")
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        mod = node.module or ""
        if mod not in IMPORT_OK:
            self.add("GUI1", node, f"from {mod} import ...")
        if mod == "ctypes":
            for alias in node.names:
                if alias.name in NAMES:
                    self.add(NAMES[alias.name], node, f"from ctypes import {alias.name}")
                elif alias.name in ESCAPES or alias.name == "*":
                    self.add("GUI1", node, f"from ctypes import {alias.name}")
        self.generic_visit(node)

    def visit_Attribute(self, node: ast.Attribute) -> None:
        self.name_hit(node.attr, node)
        if node.attr.startswith("_"):
            # private and dunder attributes reach the runtime's own unguarded handles (raw.x11._lib, type internals)
            self.add("GUI2", node, f"private attribute {node.attr}")
        elif node.attr in ESCAPES:
            self.add("GUI1", node, f"attribute {node.attr}")
        self.generic_visit(node)

    def visit_Name(self, node: ast.Name) -> None:
        if node.id in {"getattr", "setattr", "delattr", "hasattr"} and not getattr(
            node, "_called", False
        ):
            # an alias (g = getattr) would hide every later lookup from this check
            self.add("GUI2", node, f"{node.id} used as a value")
        if node.id in DYNAMIC:
            self.add("GUI2", node, node.id)
        elif node.id in ESCAPES:
            self.add("GUI1", node, f"name {node.id}")
        elif node.id in FILE_CALLS:
            self.add("GUI1", node, f"{node.id}() reads or writes files")
        elif node.id in NAMES and NAMES[node.id] != "GUI1":
            self.name_hit(node.id, node)
        if node.id in ACTIONS:
            self.last_action = max(self.last_action, _pos(node))
        if node.id in READS:
            self.last_read = max(self.last_read, _pos(node))
        self.generic_visit(node)

    def visit_Constant(self, node: ast.Constant) -> None:
        value = node.value
        if isinstance(value, bytes):
            value = value.decode("latin-1")
        if isinstance(value, str):
            for needle, rule in STRINGS.items():
                if needle in value:
                    self.add(rule, node, f"string {needle!r}")
            if value in NAMES and NAMES[value] != "GUI1":
                self.add(NAMES[value], node, f"name {value!r} in a string")
            if value.startswith("_") and value.strip("_") and value.isidentifier():
                self.add("GUI2", node, f"private name {value!r} in a string")
        self.generic_visit(node)

    def visit_Call(self, node: ast.Call) -> None:
        if isinstance(node.func, ast.Name):
            node.func._called = True  # type: ignore[attr-defined]
        func = _dotted(node.func)
        last = func.rsplit(".", 1)[-1]
        if (
            last in {"getattr", "setattr", "delattr", "hasattr"}
            and len(node.args) >= 2
            and last != "getattr"
        ):
            self.add("GUI2", node, f"{last}")
        if last == "escalate_foreground":
            reason = _const_str(node.args[0]) if node.args else None
            if reason and reason.strip():
                self.escalated = True
                self.escalation = reason.strip()
            else:
                self.add("GUI2", node, "raw.escalate_foreground needs a literal reason string")
        if last == "getattr" and len(node.args) >= 2:
            attr = _const_str(node.args[1])
            if attr is None:
                self.add("GUI2", node, "getattr with a computed name")
            else:
                self.name_hit(attr, node)
                if attr.startswith("_") or attr in ESCAPES:
                    self.add("GUI2", node, f"getattr {attr!r}")
        if last == "xdotool":
            self.xdotool(node)
        self.generic_visit(node)

    def xdotool(self, node: ast.Call) -> None:
        args = [_const_str(a) for a in node.args]
        if not args or args[0] is None:
            self.add(
                "GUI2",
                node,
                "raw.xdotool with a computed subcommand (the runtime checks it again)",
            )
            return
        sub = args[0]
        commands = xdotool_commands(args)
        if len(commands) > 1:
            self.add(
                "BG9", node, "xdotool command chain (" + " ".join(c for c, _ in commands) + ")"
            )
        for command, windowed in commands:
            rule = XDOTOOL.get(command)
            if rule == "BG3" and windowed:
                self.add("BG6", node, f"xdotool {command} --window (XSendEvent)")
            elif rule:
                self.add(rule, node, f"xdotool {command}")
        if any(c in {"key", "type", "click", "mousedown", "mouseup"} for c, _ in commands):
            self.last_action = max(self.last_action, _pos(node))


def lint(source: str) -> Report:
    """Findings for one script. A syntax error is returned as the caller's problem, not a finding."""
    tree = ast.parse(source, "<script>", "exec")
    walker = _Walker()
    walker.visit(tree)
    if walker.last_action > (0, 0) and walker.last_read <= walker.last_action:
        walker.add(
            "VR1",
            ast.Constant(value=None, lineno=walker.last_action[0]),
            "no read after the last action on this line",
        )
    return Report(walker.findings, walker.escalated, walker.escalation)


def rules_markdown() -> str:
    """The rule table, for the report and the tool description."""
    lines = ["| id | rule | severity | source | instead |", "|---|---|---|---|---|"]
    for r in RULES.values():
        lines.append(f"| {r.id} | {r.title} | {r.severity} | {r.source} | {r.instead} |")
    return "\n".join(lines)
