# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Static background-use lint for raw-macos scripts (the `pi-raw-macos` bench arm).

A script is Python that drives macOS through the `raw` runtime: the Accessibility API (AXUIElement through pyobjc),
per-process event posting (CGEventPostToPid), CoreGraphics screenshots and read-back helpers. Nothing else: the arm
has no Cua Driver, and any call path to it (``raw.driver``, its CLI, its socket) is blocked (rule DRV1). Before it
runs, `lint(source)` walks its AST and returns findings. Each finding names a rule, the line, what the rule
protects, where Cua Driver's macOS contract states it, and a background-safe alternative.

Severity:
* ``block``: the script does not run (the GUI-only side doors, code the lint cannot read).
* ``granted``: blocked unless the harness granted foreground to this trial (the task definition's
  ``foreground_grant``, in the foreground condition). The grant is an explicit authorization, given per task by
  the harness, never by the model.
* ``warn``: the script runs; the finding is returned with the result.

The lint cannot prove a script safe (an app may activate itself, a name may be computed), so the runtime in
``raw_mac.py`` checks the same rules again when a guarded function is called, and its hook samples the frontmost
app, the key window, the real cursor, the focused element and the top window during the run.

Threat model: an honest model that does not know the background contract, or forgets it. The checker tells it, and
counts what was blocked and what slipped through. It is not a security boundary against a hostile script; the bench
guest is disposable and the GUI-only guard still watches for side doors.
"""

from __future__ import annotations

import ast
import re
from dataclasses import dataclass

DRV = "libs/cua-driver/rust"
MACOS_MD = f"{DRV}/Skills/cua-driver/MACOS.md"
MAC = f"{DRV}/crates/platform-macos/src"
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
            "MB1",
            "real cursor motion or a global pointer event",
            "granted",
            "CGEventPost to the HID or session tap, CGWarpMouseCursorPosition and CGAssociateMouseAndMouseCursorPosition "
            "move or press the user's real pointer and land on whatever window is under it.",
            f"{MACOS_MD}:48-53 (cliclick and CGEventPost at the HID tap move the real cursor); {MACOS_MD}:237-242 "
            f"(self-check 2: does this move the user's real cursor?); {MACOS_MD}:692-700 (desktop-target pointer input "
            "over a background app is refused unless foreground is authorized)",
            "Use the element's AX action (raw.ax.perform(el, 'AXPress')) or set its value (raw.set_text); for a pixel "
            "click on a background window use raw.post_click(pid, x, y), which posts to that process only.",
        ),
        Rule(
            "MB2",
            "activate, raise or make an app frontmost",
            "granted",
            "NSRunningApplication.activate, AXRaise, setting AXFrontmost, AXMain or AXFocused on a window or app, "
            "SetFrontProcess and NSApp.activate bring the target in front of the user's app.",
            f"{MACOS_MD}:9-25 (the no-foreground contract: background actions must preserve the user's frontmost "
            f"app); {MACOS_MD}:56-57 (NSRunningApplication.activate from a helper is forbidden); {MAC}/focus_steal.rs:4-14",
            "Act on the element in its background window through AX (AXPress, AXValue); none of these needs the app in "
            "front.",
        ),
        Rule(
            "MB3",
            "move keyboard focus",
            "granted",
            "Setting AXFocused on an element, or AXFocusedUIElement on an app, moves key focus; for a background app "
            "AppKit can make its window key and take the user's keystrokes.",
            f"{MACOS_MD}:619-630 (background dispatch keeps the user's front window key); {MAC}/key_focus_guard.rs:4-19 "
            "(the driver guards and hands back key focus the target takes)",
            "Set the field's text with raw.set_text(el, text): it writes AXValue, and only if the field ignores that it "
            "focuses the field inside its own background app and types; a change to the user's front app, key window "
            "or focused element is refused under this rule.",
        ),
        Rule(
            "MB4",
            "input to the user's app",
            "granted",
            "Keys or clicks posted to the user's own app (the frontmost app the task says to leave alone) land in "
            "their window: their typing is interrupted or changed.",
            f"{MACOS_MD}:9-25 (background actions must preserve the user's frontmost app); {MACOS_MD}:997 (keys are "
            "posted to the exact window addressed, which is made key without a raise)",
            "Post to the target app's pid (raw.post_key(target_pid, ...)) or set the field's text (raw.set_text).",
        ),
        Rule(
            "MB5",
            "launch or open through LaunchServices, AppleScript or a shell",
            "block",
            "Every form of open, NSWorkspace open/launch, osascript, NSAppleScript and System Events activate the "
            "target; they are also outside the GUI.",
            f"{MACOS_MD}:27-47 (every form of open activates); {MACOS_MD}:54-55 (NSAppleScript, osascript); "
            "libs/cua-driver-bench/automated-eval/macos_bench/bench_v2.py:188-211 (EXEC_PATTERNS, the GUI-only guard)",
            "The task's apps are already running: find them with raw.apps() or raw.windows().",
        ),
        Rule(
            "MB6",
            "menu bar of a background app",
            "granted",
            "The visible menu bar belongs to the frontmost app; pressing a background app's AXMenuBarItem opens a "
            "menu that takes key focus and raises its window, and its items may be disabled.",
            f"{MACOS_MD}:83-93 (the AXMenuBar rule); {MACOS_MD}:632-649 (menus opened in the background take key focus "
            "and lift the window)",
            "Press an in-window control with the same command, or run the menu command with "
            "raw.menu(pid, ['File', 'Save']), which finds the item in the menu bar without opening a menu and "
            "presses it.",
        ),
        Rule(
            "MB7",
            "open a popup, pull-down or context menu",
            "granted",
            "AXShowMenu, AXPress on a pop-up or menu button, and a posted right-click or a posted click on such a "
            "button open a menu that steals key focus from the user's window and lifts the target window over it while "
            "it is open.",
            f"{MACOS_MD}:632-649; {MAC}/key_focus_guard.rs:4-28",
            "Choose a web <select> with raw.select(el, 'Label'): it types the label into the closed control and the "
            "menu never opens. A native AppKit pop-up lists its items only while its menu is open, so raw.select on "
            "one opens it and needs the trial's grant; without it there is no background route.",
        ),
        Rule(
            "MB8",
            "clipboard write",
            "warn",
            "Writing NSPasteboard replaces what the user copied.",
            f"{MACOS_MD}:864-870 (clipboard_write only as an explicit fallback for text)",
            "Set text with AXValue instead.",
        ),
        Rule(
            "MB9",
            "focus-taking or tab-switching shortcuts",
            "warn",
            "cmd+L (omnibox), cmd+shift+G, cmd+1..9 and cmd+[ ] make the receiving app activate itself or visibly "
            "switch tabs even when posted to a background pid.",
            f"{MACOS_MD}:61-77",
            "Set the field's AXValue, or press the control that does the same.",
        ),
        Rule(
            "DRV1",
            "Cua Driver",
            "block",
            "This arm uses only macOS's own APIs. Cua Driver (raw.driver, its CLI or its daemon socket) is not part "
            "of it.",
            "the arm's definition (raw macOS APIs only; tools/raw_macos/raw_mac.py)",
            "Use raw.ax, raw.find / raw.dump / raw.el, raw.set_text, raw.select, raw.set_cell, raw.menu and the "
            "per-process events.",
        ),
        Rule(
            "MB11",
            "Spaces and window-server private calls",
            "block",
            "SkyLight/CGS private functions switch Spaces, reorder windows of other apps or steal key focus; the raw "
            "runtime does not expose them. One exception: SLPSReleaseKeyFocusWithID, which hands key focus back to the "
            'user\'s app, is allowed (Zane, 13 Nov 2026: "lets allow it still since it is a macos api"); raw.select '
            "makes that call itself for a background native pop-up.",
            f"{MAC}/input/skylight.rs:4-20 (the driver's one SkyLight path: per-pid posting with an authentication "
            f"envelope); {MACOS_MD}:619-630 (its focus-without-raise recipe); {MACOS_MD}:800-803 (do not inject raw "
            f"CGEvents or use shell activation); {MAC}/key_focus_guard.rs:4-19 (the released call)",
            "Use AX and raw.post_click / raw.post_key, which post to one process; choose a native pop-up with "
            "raw.select, which releases the menu's key-focus theft for you.",
        ),
        Rule(
            "GUI1",
            "code or files outside the GUI",
            "block",
            "Bench v2 is GUI-only: no shell, process, file, network or library loading from a computer-use script.",
            "libs/cua-driver-bench/automated-eval/macos_bench/bench_v2.py:188-211 (EXEC_PATTERNS, the GUI-only "
            "guard)",
            "Use only the raw runtime: raw.ax, raw.post_click / post_key, raw.windows(), raw.screenshot().",
        ),
        Rule(
            "GUI2",
            "dynamic code the lint cannot read",
            "block",
            "eval/exec/compile, __import__/importlib, getattr with a computed name, globals()/vars() and private "
            "attributes hide the call from this check.",
            "this lint (its rules are only enforceable on code it can read)",
            "Call the API by its plain name.",
        ),
        Rule(
            "VR1",
            "action without a read-back",
            "warn",
            "An action that the API accepted is not a confirmed effect.",
            f"{CONTRACT}:149-152 (native API acceptance alone does not justify confirmed); {MACOS_MD}:185-199 "
            "(verify by re-reading the tree)",
            "After the last action, read the element back (raw.ax.get(el, 'AXValue'), raw.describe, raw.dump, "
            "raw.text) or take raw.screenshot() and return it.",
        ),
    )
}

# Function and attribute names, matched on the final name anywhere in the script (a call, an alias or a getattr with
# a constant). Exact names: no fuzzy matching.
NAMES: dict[str, str] = {
    **dict.fromkeys(
        (
            "CGEventPost",
            "CGWarpMouseCursorPosition",
            "CGAssociateMouseAndMouseCursorPosition",
            "CGDisplayMoveCursorToPoint",
            "pointer_move",
            "pointer_click",
            "pointer_drag",
        ),
        "MB1",
    ),
    **dict.fromkeys(
        (
            "activateWithOptions_",
            "activate",
            "activateIgnoringOtherApps_",
            "SetFrontProcess",
            "SetFrontProcessWithOptions",
            "AXRaise",
            "raise_window",
            "unhide",
            "unhideWithoutActivation",
        ),
        "MB2",
    ),
    **dict.fromkeys(
        (
            "NSAppleScript",
            "NSTask",
            "launchApplication_",
            "openURL_",
            "openApplicationAtURL_configuration_completionHandler_",
            "openURLs_withApplicationAtURL_configuration_completionHandler_",
            "launchAppWithBundleIdentifier_options_additionalEventParamDescriptor_launchIdentifier_",
        ),
        "MB5",
    ),
    "AXShowMenu": "MB7",
    "driver": "DRV1",
    **dict.fromkeys(
        ("setString_forType_", "writeObjects_", "clearContents", "setData_forType_"), "MB8"
    ),
    **dict.fromkeys(
        ("CDLL", "cdll", "PyDLL", "pydll", "LoadLibrary", "dlopen", "loadBundle"), "GUI1"
    ),
    **dict.fromkeys(("system", "popen", "spawnv", "execv", "execvp", "fork"), "GUI1"),
}
# AX attributes whose write is a focus or activation request (raw.ax.set(el, NAME, ...)).
AX_SET: dict[str, str] = {
    "AXFrontmost": "MB2",
    "AXMain": "MB2",
    "AXFocused": "MB3",
    "AXFocusedUIElement": "MB3",
    "AXFocusedWindow": "MB2",
    "AXMainWindow": "MB2",
    "AXHidden": "MB2",
    "AXMinimized": "MB2",
}
AX_ACTION: dict[str, str] = {"AXRaise": "MB2", "AXShowMenu": "MB7"}
# The one SkyLight call MB11 permits (Zane, 13 Nov 2026). A string naming exactly it is not a finding; any other
# SkyLight or CGS name still is.
MB11_ALLOWED = ("SLPSReleaseKeyFocusWithID",)
# Text that names a forbidden route wherever it appears in a string.
STRINGS: dict[str, str] = {
    "osascript": "MB5",
    "System Events": "MB5",
    "tell application": "MB5",
    "/usr/bin/open": "MB5",
    "SkyLight": "MB11",
    "CGSSetActiveSpace": "MB11",
    "SLSSetActiveSpace": "MB11",
    "SLPS": "MB11",
    "cua-driver": "DRV1",
    "cua_driver": "DRV1",
    "CuaDriver": "DRV1",
    "CGSOrderWindow": "MB11",
    "SLSOrderWindow": "MB11",
    "_SLPSSetFrontProcess": "MB11",
}
# Key combos that ask the receiving app to activate or switch tabs (MACOS.md:61-77).
FOCUS_COMBOS = {
    ("cmd", "l"),
    ("cmd", "shift", "g"),
    ("cmd", "]"),
    ("cmd", "["),
    ("cmd", "shift", "]"),
    ("cmd", "shift", "["),
    *{("cmd", str(n)) for n in range(1, 10)},
}
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
# Names that reach past the runtime: pyobjc and the frameworks themselves, the CPython C API, the loader.
ESCAPES = {
    "objc",
    "AppKit",
    "Quartz",
    "Foundation",
    "ApplicationServices",
    "CoreFoundation",
    "ScriptingBridge",
    "pythonapi",
    "PYFUNCTYPE",
    "CFUNCTYPE",
    "cast",
    "memmove",
    "memset",
    "addressof",
    "LibraryLoader",
    "lookUpClass",
    "from_address",
    "from_buffer",
    "in_dll",
    "pyobjc_unicode",
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
    "perform",
    "set",
    "post_click",
    "post_key",
    "post_text",
    "post_scroll",
    "pointer_drag",
    "pointer_move",
    "set_text",
    "select",
    "set_cell",
    "menu",
}
READS = {
    "get",
    "names",
    "actions",
    "describe",
    "dump",
    "find",
    "screenshot",
    "windows",
    "apps",
    "front",
    "cursor",
    "children",
    "focused",
    "text",
    "el",
}


@dataclass(frozen=True)
class Finding:
    rule: str
    line: int
    detail: str

    @property
    def severity(self) -> str:
        return RULES[self.rule].severity

    def render(self, granted: bool) -> str:
        r = RULES[self.rule]
        tag = {
            "block": "BLOCKED",
            "granted": "FOREGROUND (granted by the task)"
            if granted
            else "BLOCKED (this trial does not grant foreground)",
            "warn": "WARNING",
        }[r.severity]
        return (
            f"{tag} [{r.id} {r.title}] line {self.line}: {self.detail}\n"
            f"  rule: {r.why}\n  source: {r.source}\n  instead: {r.instead}"
        )


@dataclass
class Report:
    findings: list[Finding]
    granted: bool  # the harness's per-trial foreground grant, not anything the script says

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
        if not self.findings:
            return ""
        head = (
            "Background-use lint: the script was NOT run."
            if not self.ok
            else "Background-use lint: the script ran with these findings."
        )
        body = "\n".join(
            f.render(self.granted) for f in sorted(self.findings, key=lambda f: f.line)
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


def combo(keys: list[str]) -> tuple[str, ...]:
    """A key combo in the order FOCUS_COMBOS uses: modifiers (cmd, shift) then the key, lower-case."""
    mods = [k for k in ("cmd", "shift") if k in keys]
    rest = [k for k in keys if k not in ("cmd", "shift", "command")]
    if "command" in keys and "cmd" not in mods:
        mods.insert(0, "cmd")
    return tuple(mods + rest)


def _pos(node: ast.AST) -> tuple[int, int]:
    return getattr(node, "lineno", 0), getattr(node, "col_offset", 0)


class _Walker(ast.NodeVisitor):
    def __init__(self) -> None:
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

    def name_hit(self, name: str, node: ast.AST) -> None:
        rule = NAMES.get(name)
        if rule == "GUI1" and name in {"system", "popen", "fork", "execv", "execvp", "spawnv"}:
            if not isinstance(node, ast.Attribute):
                return  # only os.system, not a local called "system"
        if rule == "MB2" and name == "activate" and not isinstance(node, ast.Attribute):
            return  # a local called "activate"
        if rule == "DRV1" and isinstance(node, ast.Name):
            return  # a local called "driver"; raw.driver and getattr(raw, "driver") still hit
        if rule:
            self.add(rule, node, name)

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            if alias.name not in IMPORT_OK:
                self.add("GUI1", node, f"import {alias.name}")
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if (node.module or "") not in IMPORT_OK:
            self.add("GUI1", node, f"from {node.module} import ...")
        self.generic_visit(node)

    def visit_Attribute(self, node: ast.Attribute) -> None:
        self.name_hit(node.attr, node)
        if node.attr.startswith("_"):
            self.add("GUI2", node, f"private attribute {node.attr}")
        elif node.attr in ESCAPES:
            self.add("GUI1", node, f"attribute {node.attr}")
        if node.attr in ACTIONS and _dotted(node.value) in {"raw", "raw.ax"}:
            self.last_action = max(self.last_action, _pos(node))
        if node.attr in READS and _dotted(node.value) in {"raw", "raw.ax"}:
            self.last_read = max(self.last_read, _pos(node))
        self.generic_visit(node)

    def visit_Name(self, node: ast.Name) -> None:
        if node.id in {"getattr", "setattr", "delattr", "hasattr"} and not getattr(
            node, "_called", False
        ):
            self.add("GUI2", node, f"{node.id} used as a value")
        if node.id in DYNAMIC:
            self.add("GUI2", node, node.id)
        elif node.id in ESCAPES:
            self.add("GUI1", node, f"name {node.id}")
        elif node.id in FILE_CALLS:
            self.add("GUI1", node, f"{node.id}() reads or writes files")
        elif node.id in NAMES and NAMES[node.id] != "GUI1":
            self.name_hit(node.id, node)
        self.generic_visit(node)

    def visit_Constant(self, node: ast.Constant) -> None:
        value = node.value
        if isinstance(value, bytes):
            value = value.decode("latin-1")
        if isinstance(value, str):
            # only the exact name is exempt: a longer identifier that contains it is still checked
            value = re.sub(
                r"[A-Za-z0-9_]+", lambda m: "" if m.group(0) in MB11_ALLOWED else m.group(0), value
            )
            for needle, rule in STRINGS.items():
                if needle in value:
                    self.add(rule, node, f"string {needle!r}")
            if value in NAMES and NAMES[value] != "GUI1" and value not in ("activate", "driver"):
                self.add(NAMES[value], node, f"name {value!r} in a string")
            if value.startswith("_") and value.strip("_") and value.isidentifier():
                self.add("GUI2", node, f"private name {value!r} in a string")
        self.generic_visit(node)

    def visit_Call(self, node: ast.Call) -> None:
        if isinstance(node.func, ast.Name):
            node.func._called = True  # type: ignore[attr-defined]
        func = _dotted(node.func)
        last = func.rsplit(".", 1)[-1]
        if last in {"setattr", "delattr", "hasattr"} and len(node.args) >= 2:
            self.add("GUI2", node, last)
        if last == "getattr" and len(node.args) >= 2:
            attr = _const_str(node.args[1])
            if attr is None:
                self.add("GUI2", node, "getattr with a computed name")
            else:
                self.name_hit(attr, node)
                if attr.startswith("_") or attr in ESCAPES:
                    self.add("GUI2", node, f"getattr {attr!r}")
        if func == "raw.ax.set" and len(node.args) >= 2:
            attr = _const_str(node.args[1])
            if attr in AX_SET:
                self.add(AX_SET[attr], node, f"raw.ax.set(..., {attr!r})")
        if func == "raw.ax.perform" and len(node.args) >= 2:
            action = _const_str(node.args[1])
            if action in AX_ACTION:
                self.add(AX_ACTION[action], node, f"raw.ax.perform(..., {action!r})")
        if func == "raw.post_click":
            right = len(node.args) >= 4 and _const_str(node.args[3]) == "right"
            right = right or any(
                k.arg == "button" and _const_str(k.value) == "right" for k in node.keywords
            )
            if right:
                self.add("MB7", node, "raw.post_click with button='right' opens a context menu")
        if func == "raw.post_key":
            self.keys(node)
        self.generic_visit(node)

    def keys(self, node: ast.Call) -> None:
        for child in ast.walk(node):
            if (
                isinstance(child, ast.List)
                and all(_const_str(e) is not None for e in child.elts)
                and child.elts
            ):
                keys = [str(_const_str(e)).lower() for e in child.elts]
                if combo(keys) in FOCUS_COMBOS:
                    self.add("MB9", node, "keys " + "+".join(keys))


def lint(source: str, granted: bool = False) -> Report:
    """Findings for one script on a trial with or without the harness's foreground grant. A syntax error is the
    caller's problem, not a finding."""
    tree = ast.parse(source, "<script>", "exec")
    walker = _Walker()
    walker.visit(tree)
    if walker.last_action > (0, 0) and walker.last_read <= walker.last_action:
        walker.add(
            "VR1",
            ast.Constant(value=None, lineno=walker.last_action[0]),
            "no read after the last action on this line",
        )
    return Report(walker.findings, granted)


def rules_markdown() -> str:
    """The rule table, for the report."""
    lines = ["| id | rule | severity | source |", "|---|---|---|---|"]
    for r in RULES.values():
        lines.append(f"| {r.id} | {r.title} | {r.severity} | {r.source} |")
    return "\n".join(lines)
