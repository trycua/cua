# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Static background-use lint for raw-linux scripts (the `pi-raw-linux` bench arm).

A script is Python source that drives Linux low-level APIs (AT-SPI through gi, Xlib/XTest through ctypes handles,
xdotool) through the `raw` runtime. Nothing else: the arm has no Cua Driver, and any call path to it (``raw.driver``,
its CLI, its daemon socket, its Python package) is blocked (rule DRV1). Before it runs, `lint(source)` walks its AST and returns findings. Each finding
names a rule, the line, what the rule protects, the source of the rule in Cua Driver's own contract, and a
background-safe alternative.

Severity:
* ``block``: the script does not run. Used for pointer, focus and keyboard theft (the background contract), grabs,
  and the bench's GUI-only side doors.
* ``granted``: blocked unless the harness granted foreground to this task (the task definition's
  ``foreground_grant``). The grant is an explicit authorization, given per task by the harness, never by the model.
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
    severity: str  # block | granted | warn
    why: str
    source: str
    instead: str


RULES: dict[str, Rule] = {
    r.id: r
    for r in (
        Rule(
            "BG1",
            "real pointer motion or button",
            "granted",
            "XTest, XWarpPointer, xdotool mousemove/click and Atspi.generate_mouse_event move or press the user's "
            "real (core) pointer. Background input must not move it; only an explicit desktop/foreground request does.",
            f"{INPUT_RS}:4-15 (no XTest fallback: it delivers to the focused window); "
            f"{DRV}/crates/platform-linux/src/tools/move_cursor.rs:45 (only scope=desktop moves the real pointer); "
            f"{LINUX_MD}:26-31 (foreground is an authorized escalation, never automatic)",
            "Use the element's AT-SPI Action (Atspi.Action.do_action(acc, i)), EditableText or Value interface. A real "
            "click, hover or drag (raw.click, raw.drag) is allowed only on a task that grants foreground.",
        ),
        Rule(
            "BG2",
            "focus, raise or activate",
            "granted",
            "XSetInputFocus, XRaiseWindow/XMapRaised, a _NET_ACTIVE_WINDOW or _NET_RESTACK_WINDOW request, wmctrl -a "
            "and xdotool windowactivate/windowfocus/windowraise take focus from the user's window or raise over it.",
            f"{LINUX_MD}:19-20 (background input must not activate or raise the target); "
            f"{DRV}/crates/platform-linux/src/input/focus_guard.rs:4-12 (core focus, active window and top of "
            "stacking must be left as found)",
            "Address the element directly through AT-SPI (do_action, set_text_contents, set_current_value); these "
            "need no focus. Activation (raw.activate) is allowed only on a task that grants foreground.",
        ),
        Rule(
            "BG3",
            "keys to whatever is focused",
            "granted",
            "XTestFakeKeyEvent, Atspi.generate_keyboard_event and xdotool key/type/keydown/keyup without --window "
            "deliver to the focused window, which is the user's, not the target.",
            f"{INPUT_RS}:4-15; {LINUX_MD}:29-31 (if activation cannot be proven, refuse instead of sending keys to "
            "whatever is focused)",
            "Set text with the element's EditableText (set_text_contents / insert_text) or Value interface, or press "
            "the button that the key would trigger through its AT-SPI Action. raw.key / raw.type_text into an "
            "activated window are allowed only on a task that grants foreground.",
        ),
        Rule(
            "BG4",
            "global grab",
            "block",
            "XGrabKeyboard, XGrabPointer, XGrabKey, XGrabButton and XGrabServer take every input device or the "
            "whole server away from the user until released.",
            f"{INPUT_RS}:1792-1797 (the driver dropped its XI2 shield grab: it swallowed input and left the device "
            f"frozen); {DRV}/crates/platform-linux/src/input/focus_guard.rs:24-27 (a held grab cannot be undone)",
            "No grab is needed: AT-SPI actions and EditableText reach the element without one.",
        ),
        Rule(
            "BG5",
            "Wayland or portal input to the compositor focus",
            "granted",
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
            "Use only the raw runtime: raw.Atspi, raw.x11/raw.xtst handles, raw.xdotool(...), raw.windows(), "
            "raw.screenshot().",
        ),
        Rule(
            "DRV1",
            "Cua Driver",
            "block",
            "This arm uses only Linux's own APIs and tools. Cua Driver (raw.driver, its CLI, its daemon socket or "
            "its Python package) is not part of it.",
            "the arm's definition (raw Linux APIs only; tools/raw_linux/raw.py)",
            "Use raw.Atspi (Action, EditableText, Value, Selection, Table), raw.find / raw.dump / raw.cells, and on "
            "a task that grants foreground raw.activate / raw.key / raw.type_text / raw.click / raw.scroll / "
            "raw.drag.",
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
            "granted",
            "xdotool runs every command in a chain; a later key or pointer command does not get the first "
            "command's --window and goes to the focused window or the real pointer.",
            f"{INPUT_RS}:4-15 (keys go to the addressed window, never to whatever is focused)",
            "Make one raw.xdotool call per command. Chains are allowed only on a task that grants foreground.",
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
    "driver": "DRV1",
}
# The runtime's foreground helpers (raw.<name>) and the rule each needs the grant under.
HELPERS: dict[str, str] = {
    "activate": "BG2",
    "key": "BG3",
    "type_text": "BG3",
    "click": "BG1",
    "scroll": "BG1",
    "drag": "BG1",
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
    "cua-driver": "DRV1",
    "cua_driver": "DRV1",
    "CuaDriver": "DRV1",
    "cdb-bench-cua": "DRV1",  # the bench's driver daemon sockets
}
# Every xdotool subcommand. A second subcommand in one raw.xdotool call (xdotool's command chaining) is BG9, so each
# command's own --window decides its delivery.
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
# The no-wrapper arm (pi-raw-linux-native, RAW_NATIVE=1): no raw.* runtime. Scripts import Linux's own Python
# bindings (python-xlib, gi's Atspi/Gdk, pyatspi, dbus) and ctypes for native libraries (libX11, libXtst, libxdo)
# and call them directly; xdotool runs through the one global xdotool(*args) (Zane, 13 Nov 2026 16:15 SGT). Shells,
# processes, files and network stay out (GUI1); the same background rules apply to the native names below.
NATIVE_IMPORTS = {"Xlib", "pyatspi", "dbus", "ctypes"}
NATIVE_GI = {"Atspi", "Gdk", "GdkPixbuf", "GdkX11", "GLib", "GObject"}  # gi.repository modules a script may load
NATIVE_ESCAPES = {
    "pythonapi",
    "util",  # ctypes.util.find_library runs ldconfig/gcc: a process
    "find_library",
    "PYFUNCTYPE",
    "Gio",
    "Gtk",
    "Subprocess",
    "SubprocessLauncher",
    "spawn_async",
    "spawn_sync",
    "spawn_async_with_pipes",
    "spawn_command_line_async",
    "spawn_command_line_sync",
    "file_set_contents",
    "file_get_contents",
}
NATIVE_READ_ATOMS = {"_NET_ACTIVE_WINDOW", "_NET_RESTACK_WINDOW"}  # atoms the native arm may read
NATIVE_LOADERS = {"CDLL", "cdll", "LoadLibrary", "dlopen"}
# Native libraries a script may load with ctypes: the X11 / XTest / xdo / AT-SPI / GDK input and read libraries. libc
# and the process's own symbols (CDLL(None)) are files, processes and memory, not GUI (GUI1).
NATIVE_LIBS = ("libX11", "libXtst", "libXi", "libXext", "libXfixes", "libXrandr", "libXcomposite", "libXdamage",
               "libxdo", "libatspi", "libgdk-3", "libgdk_pixbuf", "libxcb")


# Symbols a native library handle resolves: the GUI APIs' own prefixes. dlsym also finds a library's dependencies'
# symbols (libc's fopen through libX11), so the prefix, not the library, decides.
NATIVE_SYMBOL_PREFIXES = ("X", "xcb_", "xdo_", "atspi_", "gdk_")


def native_symbol_ok(name: str) -> bool:
    return name.startswith(NATIVE_SYMBOL_PREFIXES)


def native_lib_ok(name: object) -> bool:
    """A ctypes library name or path the native arm may load: one of NATIVE_LIBS, by file name."""
    if not isinstance(name, str) or not name:
        return False
    import re

    import posixpath

    if "/" in name:
        name = posixpath.normpath(name)
    folder, _, base = name.rpartition("/")
    if folder and not (folder + "/").startswith(("/usr/lib/", "/lib/")):
        return False  # a system library only, never one from a writable directory
    return any(re.fullmatch(re.escape(lib) + r"(\.so(\.\d+)*)?", base) for lib in NATIVE_LIBS)
# Native names of the same foreground routes (python-xlib, pyatspi, Gdk, libxdo), matched as in NAMES.
NATIVE_NAMES: dict[str, str] = {
    **dict.fromkeys(
        (
            "fake_input",  # python-xlib XTest: motion, buttons and keys
            "warp_pointer",
            "generateMouseEvent",
            "test_simulate_button",
            "warp",
            "xdo_move_mouse",
            "xdo_move_mouse_relative",
            "xdo_move_mouse_relative_to_window",
            "xdo_click_window",
            "xdo_click_window_multiple",
            "xdo_mouse_down",
            "xdo_mouse_up",
        ),
        "BG1",
    ),
    **dict.fromkeys(
        (
            "set_input_focus",
            "raise_window",
            "circulate",
            "raise_",
            "xdo_activate_window",
            "xdo_focus_window",
            "xdo_raise_window",
        ),
        "BG2",
    ),
    **dict.fromkeys(
        (
            "generateKeyboardEvent",
            "test_simulate_key",
            "xdo_send_keysequence_window",
            "xdo_send_keysequence_window_down",
            "xdo_send_keysequence_window_up",
            "xdo_send_keysequence_window_list_do",
            "xdo_enter_text_window",
        ),
        "BG3",
    ),
    **dict.fromkeys(
        (
            "grab_pointer",
            "grab_keyboard",
            "grab_key",
            "grab_button",
            "grab_server",
            "xtest_grab_control",
            "grab_control",
            "pointer_grab",
            "keyboard_grab",
        ),
        "BG4",
    ),
    "send_event": "BG6",
    "grabFocus": "BG7",
    "set_selection_owner": "BG8",
}


# The native arm's alternatives: the wrapper's "instead" texts name raw.* helpers the native arm does not have.
NATIVE_INSTEAD = {
    "BG1": "Use the element's AT-SPI Action (Atspi.Action.do_action(acc, i)), EditableText or Value interface. Real "
    "pointer input (XTest, xdotool mousemove/click) is allowed only on a task that grants foreground.",
    "BG2": "Address the element directly through AT-SPI (do_action, set_text_contents, set_current_value); these need "
    "no focus. Activating a window is allowed only on a task that grants foreground.",
    "BG3": "Set text with the element's EditableText or Value interface, or press the button the key would trigger "
    "through its AT-SPI Action. XTest or xdotool keys into an activated window are allowed only on a task that grants "
    "foreground.",
    "GUI1": "Use Linux's GUI APIs only: gi Atspi/Gdk, pyatspi, python-xlib, dbus, ctypes for libX11/libXtst/libxdo, "
    "and xdotool(...).",
    "DRV1": "Use AT-SPI (Action, EditableText, Value, Selection, Table) and, on a task that grants foreground, XTest "
    "or xdotool input.",
    "BG9": "Make one xdotool(...) call per command. Chains are allowed only on a task that grants foreground.",
    "VR1": "After the last action, read the element back (get_text, get_current_value, get_state_set, get_name) or "
    "save a screenshot into SHOTS.",
}


def import_ok(name: str, native: bool = False) -> bool:
    if not native:
        return name in IMPORT_OK
    if name == "raw":
        return False
    if name in IMPORT_OK or name.split(".")[0] in NATIVE_IMPORTS or name in ("gi", "gi.repository"):
        return True
    return name.startswith("gi.repository.") and name.split(".")[2] in NATIVE_GI


def private_ok(name: str, native: bool) -> bool:
    """A name starting with _ the script may use: none in the wrapper arm; in the native arm a single-underscore name
    (ctypes' _fields_, python-xlib's _data), never a dunder."""
    return native and not name.startswith("__")


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
    "key",
    "type_text",
    "click",
    "scroll",
    "drag",
}
READS = {
    "cells",
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
# the native arm's reads (python-xlib, Gdk, AT-SPI Table), counted for VR1 in that arm only
NATIVE_READS = {
    "savev",
    "get_full_property",
    "get_geometry",
    "query_pointer",
    "get_input_focus",
    "get_accessible_at",
    "get_extents",
}


@dataclass(frozen=True)
class Finding:
    rule: str
    line: int
    detail: str

    @property
    def severity(self) -> str:
        return RULES[self.rule].severity

    def render(self, granted: bool, native: bool = False, bare: bool = False) -> str:
        r = RULES[self.rule]
        if bare:  # the bare arm: which rule refused the script, no explanation or alternative
            return f"BLOCKED [{r.id} {r.title}] line {self.line}: {self.detail}"
        instead = NATIVE_INSTEAD.get(r.id, r.instead) if native else r.instead
        tag = {
            "block": "BLOCKED",
            "granted": "FOREGROUND (granted by the task)" if granted else "BLOCKED (this task does not grant foreground)",
            "warn": "WARNING",
        }[r.severity]
        return (
            f"{tag} [{r.id} {r.title}] line {self.line}: {self.detail}\n"
            f"  rule: {r.why}\n  source: {r.source}\n  instead: {instead}"
        )


@dataclass
class Report:
    findings: list[Finding]
    granted: bool  # the harness's per-task foreground grant, not anything the script says
    native: bool = False  # the no-wrapper arm: alternatives name native calls, not raw.* helpers
    bare: bool = False  # the bare arm: only blocking findings, rule id and title, no hints

    @property
    def blocked(self) -> list[Finding]:
        return [
            f
            for f in self.findings
            if f.severity == "block" or (f.severity == "granted" and not self.granted)
        ]

    @property
    def ok(self) -> bool:
        return not self.blocked

    def render(self) -> str:
        if self.bare:
            return "\n".join(
                ["The script was not run."]
                + [f.render(self.granted, self.native, True) for f in sorted(self.blocked, key=lambda f: f.line)]
            ) if self.blocked else ""
        if not self.findings:
            return ""
        head = (
            "Background-use lint: the script was NOT run."
            if not self.ok
            else "Background-use lint: the script ran with these findings."
        )
        body = "\n".join(
            f.render(self.granted, self.native) for f in sorted(self.findings, key=lambda f: f.line)
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
    def __init__(self, raw_names: set[str] | None = None, native: bool = False) -> None:
        self.native = native
        self.atom_requests: list[tuple[str, ast.AST]] = []
        self.sends = False
        self.names = {**NAMES, **NATIVE_NAMES} if native else NAMES
        self.reads = READS | NATIVE_READS if native else READS
        self.raw_names = raw_names or {"raw"}  # "raw" and every local bound to it (r = raw)
        self.findings: list[Finding] = []
        self.last_action: tuple[int, int] = (0, 0)
        self.last_read: tuple[int, int] = (0, 0)
        self.seen: set[tuple[str, int]] = set()

    def add(self, rule: str, node: ast.AST, detail: str) -> None:
        line = getattr(node, "lineno", 0)
        if (rule, line) in self.seen:
            return
        self.seen.add((rule, line))
        self.findings.append(Finding(rule, line, detail))

    def escapes(self) -> set[str]:
        return NATIVE_ESCAPES if self.native else ESCAPES

    def name_hit(self, name: str, node: ast.AST) -> None:
        rule = self.names.get(name)
        if self.native and name in NATIVE_LOADERS:
            return  # loading a native library is the native arm's point
        if rule == "GUI1" and name in {"system", "popen", "fork", "execv", "execvp", "spawnv"}:
            # only as an attribute (os.system), not a plain local called "system"
            if not isinstance(node, ast.Attribute):
                return
        if rule == "DRV1" and isinstance(node, ast.Name):
            return  # a local called "driver"; raw.driver and getattr(raw, "driver") still hit
        if rule:
            self.add(rule, node, f"{name}")
        if name in {"send_event", "XSendEvent"}:
            self.sends = True
        if name in ACTIONS:
            self.last_action = max(self.last_action, _pos(node))
        if name in self.reads:
            self.last_read = max(self.last_read, _pos(node))

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            if "cua_driver" in alias.name:
                self.add("DRV1", node, f"import {alias.name}")
            if not import_ok(alias.name, self.native):
                self.add("GUI1", node, f"import {alias.name}")
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        mod = node.module or ""
        if "cua_driver" in mod:
            self.add("DRV1", node, f"from {mod} import ...")
        if node.level or not import_ok(mod, self.native):
            self.add("GUI1", node, f"from {mod} import ...")
        if mod == "gi.repository" and self.native:
            for alias in node.names:
                if alias.name not in NATIVE_GI:
                    self.add("GUI1", node, f"from gi.repository import {alias.name}")
        if mod == "ctypes":
            for alias in node.names:
                if alias.name in self.names and not (self.native and alias.name in NATIVE_LOADERS):
                    self.add(self.names[alias.name], node, f"from ctypes import {alias.name}")
                elif alias.name in self.escapes() or alias.name == "*":
                    self.add("GUI1", node, f"from ctypes import {alias.name}")
        self.generic_visit(node)

    def visit_Attribute(self, node: ast.Attribute) -> None:
        self.name_hit(node.attr, node)
        if node.attr in HELPERS and _dotted(node.value) in self.raw_names:
            self.add(HELPERS[node.attr], node, f"raw.{node.attr}")
        if node.attr.startswith("_") and not private_ok(node.attr, self.native):
            # private and dunder attributes reach the runtime's own unguarded handles (raw.x11._lib, type internals)
            self.add("GUI2", node, f"private attribute {node.attr}")
        elif node.attr in self.escapes():
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
        elif node.id in self.escapes():
            self.add("GUI1", node, f"name {node.id}")
        elif node.id in FILE_CALLS:
            self.add("GUI1", node, f"{node.id}() reads or writes files")
        elif node.id in self.names and self.names[node.id] != "GUI1":
            self.name_hit(node.id, node)
        if node.id in ACTIONS:
            self.last_action = max(self.last_action, _pos(node))
        if node.id in self.reads:
            self.last_read = max(self.last_read, _pos(node))
        self.generic_visit(node)

    def visit_Constant(self, node: ast.Constant) -> None:
        value = node.value
        if isinstance(value, bytes):
            value = value.decode("latin-1")
        if isinstance(value, str):
            for needle, rule in STRINGS.items():
                if needle in value:
                    if self.native and needle in NATIVE_READ_ATOMS:
                        # python-xlib reads the active window through this atom; only a request sent with it
                        # (send_event / XSendEvent in the same script) activates or restacks, checked in lint()
                        self.atom_requests.append((needle, node))
                        continue
                    self.add(rule, node, f"string {needle!r}")
            if value in self.names and self.names[value] not in ("GUI1", "DRV1"):
                self.add(self.names[value], node, f"name {value!r} in a string")
            if (
                value.startswith("_")
                and value.strip("_")
                and value.isidentifier()
                and not private_ok(value, self.native)
            ):
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
        if last == "getattr" and len(node.args) >= 2:
            attr = _const_str(node.args[1])
            if attr is None:
                self.add("GUI2", node, "getattr with a computed name")
            else:
                self.name_hit(attr, node)
                if attr in HELPERS and node.args and _dotted(node.args[0]) in self.raw_names:
                    self.add(HELPERS[attr], node, f"getattr(raw, {attr!r})")
                if (attr.startswith("_") and not private_ok(attr, self.native)) or attr in self.escapes():
                    self.add("GUI2", node, f"getattr {attr!r}")
        if last == "xdotool":
            self.xdotool(node)
        if self.native and last in {"CDLL", "LoadLibrary", "dlopen"}:
            lib = _const_str(node.args[0]) if node.args else None
            if lib is None and node.args and not isinstance(node.args[0], ast.Constant):
                self.add("GUI2", node, f"{last} with a computed library name (the runtime checks it again)")
            elif not native_lib_ok(lib):
                self.add("GUI1", node, f"{last}({lib!r}): only the X11, XTest, xdo, AT-SPI and GDK libraries load")
        if self.native and last == "configure" and any(
            k.arg in (None, "stack_mode", "sibling") for k in node.keywords
        ):
            self.add("BG2", node, "configure(stack_mode=...) restacks the window")
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


def _raw_aliases(tree: ast.AST) -> set[str]:
    """"raw" and every name bound to it, directly or through another alias (r = raw; q = r; a = b = raw)."""
    names = {"raw"}
    while True:
        found = set(names)
        for node in ast.walk(tree):
            if isinstance(node, (ast.Assign, ast.AnnAssign, ast.NamedExpr)):
                value = node.value
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                if isinstance(value, ast.Name) and value.id in names:
                    found |= {t.id for t in targets if isinstance(t, ast.Name)}
        if found == names:
            return names
        names = found


def lint(source: str, granted: bool = False, native: bool = False, bare: bool = False) -> Report:
    """Findings for one script on a task with or without the harness's foreground grant. ``native``: the no-wrapper
    arm's script (native bindings and ctypes imports, single-underscore names). A syntax error is returned as the
    caller's problem, not a finding."""
    tree = ast.parse(source, "<script>", "exec")
    walker = _Walker(_raw_aliases(tree), native)
    walker.visit(tree)
    if walker.sends:
        for needle, node in walker.atom_requests:
            walker.add(STRINGS[needle], node, f"a {needle} request (send_event)")
    if walker.last_action > (0, 0) and walker.last_read <= walker.last_action:
        walker.add(
            "VR1",
            ast.Constant(value=None, lineno=walker.last_action[0]),
            "no read after the last action on this line",
        )
    return Report(walker.findings, granted, native, bare and native)


def rules_markdown() -> str:
    """The rule table, for the report and the tool description."""
    lines = ["| id | rule | severity | source | instead |", "|---|---|---|---|---|"]
    for r in RULES.values():
        lines.append(f"| {r.id} | {r.title} | {r.severity} | {r.source} | {r.instead} |")
    return "\n".join(lines)
