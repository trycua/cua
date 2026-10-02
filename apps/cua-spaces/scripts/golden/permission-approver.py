#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""permission-approver.py — approve macOS permission dialogs inside a Space.

WHAT THIS IS FOR
================
Pre-granting individual permissions has repeatedly failed to cover the real
cases, and each miss costs 20+ minutes of a run sitting in a poll loop in front
of a dialog nobody can answer. Two classes cannot be pre-granted at all:

  * LOCAL NETWORK ACCESS IS NOT A TCC SERVICE. There is no kTCCServiceLocal*
    in tccd's string table; grants live in
    /var/db/com.apple.networkextension.tracker-info, an opaque nehelper-managed
    store. No TCC.db row will pre-grant it.
  * UNKNOWN FUTURE CALLERS cannot be enumerated. A microphone prompt was raised
    by a helper whose display name was a bare string of digits; seed-tcc.sh
    grants mic/camera to Unity.app / Unity Hub.app / Blender.app, and a helper
    with its own code identity inherits none of those.

So this is the general fallback: notice a permission dialog, press the
approving button, whatever asked. Pre-grants stay the first line of defence
(seed-tcc.sh) — this catches what they miss.

THIS MUST NEVER RUN ON A HOST
=============================
It auto-approves permission prompts, which on a real machine would be a
privilege-escalation gadget: anything that can raise a TCC prompt would get the
permission it asked for, silently. It is appropriate ONLY inside a disposable,
single-user Space VM whose entire purpose is to act as the user and which is
destroyed after the run.

That boundary is enforced, not just documented — see require_disposable_space()
below. The process refuses to start unless it is running as the Space user
inside an Apple Virtual Machine that carries the Space marker file.

HOW IT DRIVES
=============
Through cua-driver's own MCP (`cua-driver-local mcp --socket …`, stdio),
which forwards to the signed CuaDriverLocal.app daemon — the component that
ALREADY holds Accessibility and Screen Recording (com.trycua.driver.local is
seeded into the system TCC db by seed-tcc.sh). This script is an unprivileged
MCP client: it touches no AX API, so it needs no TCC grant of its own, and it
adds no new privileged identity to the image.

Specifically NOT AppleScript / System Events. pywinctl (a dependency of the
since-removed computer-server) was dropped from the image precisely because its
System Events calls raised their own unanswerable two-minute Automation prompt
(see seed-tcc.sh). An
auto-approver that itself raises a permission prompt would be a bad joke.

cua-driver's click(element_token) path is an AX action — AXUIElementPerformAction
with "AXPress" (crates/platform-macos/src/input/ax_actions.rs) — NOT a synthetic
CGEvent. That distinction is the whole reason this works. The claim in
seed-tcc.sh that "TCC prompts ... deliberately IGNORE synthetic clicks" is about
posted mouse events; it does not apply to an AX press from a process holding
Accessibility (measured on the golden image).

WHAT IT CANNOT DO
=================
SecurityAgent dialogs ("<App> wants to use the 'login' keychain", the
authorization panels). SecurityAgent runs in its own secure session: its
windows are not in any AX tree this can reach and macOS ignores input to it.
Those are eliminated at the source instead: the golden build re-keys the login
keychain to the account password (sanitize-golden.sh), so no unanswerable
keychain panel is raised in the first place. They are deliberately NOT attempted
here — but they ARE detected and logged
loudly, so a stuck run says why it is stuck instead of spinning silently.

Every approval is logged: what asked, what the dialog said, which button was
pressed. A run can then report honestly what it approved rather than hiding it.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time

# ---------------------------------------------------------------------------
# Window discovery
# ---------------------------------------------------------------------------
# NOT cua-driver's list_windows, or at least not only it. list_windows is
# DELIBERATELY layer-0 only (windows.rs: `if layer != 0 && layers ==
# LayerFilter::ZeroOnly { skip }`), because surfacing every tooltip, popover,
# NSMenu and the Dock would swamp its normal callers. An alert panel raised
# modally does not have to be on layer 0, so a permission dialog can be
# invisible to it — discovery would silently find nothing and the approver
# would look healthy while approving nothing.
#
# CGWindowListCopyWindowInfo has no such filter and, importantly, needs NO
# permission for the metadata used here: window id, owner pid, owner name,
# bounds and layer are unprivileged. (Only kCGWindowName — the title — is gated
# behind Screen Recording, and it is not needed: the dialog's text comes from
# the AX tree via the driver, which does hold that grant.)
#
# get_window_state's window lookup is NOT layer-filtered — that asymmetry is
# deliberate in the driver ("Keeping the layer filter on enumeration and off
# identity lookup is what lets get_window_state tell 'no such window' apart
# from 'exists, but is not a layer-0 window'"). So a (pid, window_id) found by
# CGWindowList can be handed straight to it whatever layer it is on.
try:
    import Quartz  # type: ignore

    HAVE_QUARTZ = True
except ImportError:  # pragma: no cover - depends on the interpreter in use
    HAVE_QUARTZ = False

DRIVER_BIN = os.environ.get(
    "CUA_APPROVER_DRIVER_BIN",
    "/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local",
)
DRIVER_SOCKET = os.environ.get(
    "CUA_APPROVER_DRIVER_SOCKET",
    os.path.expanduser("~/Library/Caches/cua-driver-local/cua-driver-local.sock"),
)
POLL_SECONDS = float(os.environ.get("CUA_APPROVER_POLL_SECONDS", "2"))

# How many times to try one dialog before declaring it undrivable. A dialog we
# cannot press must become a LOUD LOG LINE, not an infinite click loop.
MAX_ATTEMPTS = 4

# ---------------------------------------------------------------------------
# Matching
# ---------------------------------------------------------------------------
# Two independent gates, and a candidate needs BOTH:
#   (1) the window looks like a permission REQUEST — either it is drawn by a
#       known prompt-drawing process, or its text matches a permission phrase;
#   (2) it carries a button from APPROVE_BUTTONS.
# Gate (1) is what keeps this from pressing "OK" on an ordinary app dialog —
# a Unity "Save changes?" sheet has an OK button too, and pressing it would be
# an unrequested edit to the user's work.

# Processes macOS uses to draw permission prompts. UserNotificationCenter draws
# the TCC alerts (microphone, camera, screen recording, files-and-folders);
# ControlCenter and sharingd draw some of the newer ones; the local-network
# prompt is drawn by the REQUESTING app itself, which is why the text gate
# below exists as an alternative.
PROMPT_PROCESSES = {
    "UserNotificationCenter",
    "universalaccessAuthWarn",
    "universalAccessAuthWarn",
    "tccd",
    "ControlCenter",
    "sharingd",
    "coreauthd",
    "NetworkExtension",
    "nehelper",
}

# CORRECTION, MEASURED. The received wisdom — repeated in seed-tcc.sh,
# prepare-space.sh and the golden-image doc — is that SecurityAgent "runs in a
# separate secure session, does not appear in the accessibility tree, and macOS
# ignores synthetic input to it". THE FIRST TWO CLAUSES ARE FALSE.
#
# Measured on a booted Space (cua-golden, macOS 26.3), against the live
# "Spotlight wants to use the “cua” keychain." panel:
#
#   LAYER   PID  WINDOW  APP             AX ELEMENTS
#    1000   599      38  SecurityAgent   5
#     - [0] AXWindow
#     - [1] AXButton (Help)
#     - [2] AXButton "OK"
#     - AXStaticText = "Spotlight wants to use the “cua” keychain."
#     - AXTextField (the password field)
#     - [4] AXButton "Cancel"
#
# It is fully in the AX tree, and an AXPress on its Cancel button DISMISSED IT
# (the window id changed from 38 to 47 as Spotlight immediately re-asked). So
# SecurityAgent is reachable AND drivable from a process holding Accessibility.
#
# These are CREDENTIAL gates, not consent gates: the panel wants a password
# typed into that AXTextField, so pressing "OK" on an empty field accomplishes
# nothing and "Cancel" is a denial.
#
# An earlier revision detected these and deliberately did NOT answer them,
# reasoning that fixing the cause beats answering the symptom. That was the
# wrong trade in practice. The cause keeps coming back wearing a different
# daemon's name (Spotlight, then assistantd), there is a ~50s window at login
# in which no in-Space machinery can run at all, and the symptom silently
# burns whole runs — one panel sat on screen for 23 minutes.
#
# We know this keychain's password: the Space creates it. So ANSWER the panel —
# type the password and approve — and if that cannot be done, fail LOUDLY so the
# agent driving the Space can see it and act, rather than leaving it to discover
# a stalled desktop on its own.
#
# Only keychain panels are answered. A panel asking for anything else (an Apple
# ID, a login password) gets the loud report and nothing typed.
CREDENTIAL_PROCESSES = {"SecurityAgent", "loginwindow", "authd"}

PERMISSION_PHRASES = [
    # TCC, current and historical phrasings.
    r"would like to access",
    r"would like to use",
    r"wants to access",
    r"wants to use",
    r"requesting access",
    r"is requesting to",
    r"requests access to",
    r"to access the (microphone|camera)",
    r"access your (microphone|camera|screen|photos|contacts|calendar|reminders)",
    # Local network — not TCC, phrased differently, drawn by the asking app.
    r"find and connect to devices on your local network",
    r"local network",
    # Screen recording / capture.
    r"to record (this computer'?s screen|the contents of your screen)",
    r"bypass the system private window picker",
    # Input monitoring / accessibility.
    r"control this (mac|computer)",
    r"monitor input from the keyboard",
    # Generic tail: the sentence every TCC alert ends with.
    r"you can (change|allow) this in (system settings|privacy)",
]
PERMISSION_RE = re.compile("|".join(PERMISSION_PHRASES), re.IGNORECASE)

# Preference order. STRONGEST GRANT FIRST, and never a denial.
#
# "Approve, don't just dismiss": pressing "Don't Allow" bakes a sticky
# auth_value = 0 row into TCC.db that is INHERITED BY THE IMAGE and is strictly
# worse than the prompt — it silently refuses the same request forever, with no
# dialog to explain it. So denial buttons are not merely un-preferred, they are
# forbidden (DENY_BUTTONS), and a dialog offering only denials is left alone and
# logged.
#
# "Allow Once" is ranked below the persistent grants but above OK: a one-shot
# approval still unblocks the run, and the next prompt gets approved too.
APPROVE_BUTTONS = [
    "always allow",
    "allow while using app",
    "allow on this network",  # local network
    "allow",
    "allow once",
    "open system settings",  # never: see below — kept out of the list
    "ok",
    "continue",
]
# "Open System Settings" is deliberately removed: it is not an approval, it
# dismisses the alert and opens a Settings window that then sits on screen for
# the rest of the run. Filtered here rather than in the list above so the
# reason stays attached to it.
APPROVE_BUTTONS = [b for b in APPROVE_BUTTONS if b != "open system settings"]

DENY_BUTTONS = {
    "don't allow",
    "dont allow",
    "deny",
    "cancel",
    "not now",
    "quit",
    "quit & reopen",
    "never",
    "no",
}

LOG_PATH = os.path.expanduser("~/.cua/permission-approver.log")


def log(msg: str) -> None:
    line = f"{time.strftime('%Y-%m-%d %H:%M:%S')} {msg}"
    print(line, flush=True)
    try:
        os.makedirs(os.path.dirname(LOG_PATH), exist_ok=True)
        with open(LOG_PATH, "a") as fh:
            fh.write(line + "\n")
    except OSError:
        pass


# ---------------------------------------------------------------------------
# Host guard
# ---------------------------------------------------------------------------
def require_disposable_space() -> None:
    """Refuse to run anywhere but inside a disposable Space VM.

    Three independent conditions, all required. Any one of them alone would be
    a weak check; together they cannot plausibly hold on somebody's laptop.

      1. The hardware is an Apple Virtual Machine. A Lume/Virtualization.framework
         guest reports hw.model = "VirtualMac2,1" (or "Apple Virtual Machine").
         A real Mac reports "MacBookPro18,3", "Mac14,6", etc.
      2. The Space marker exists (~/.cua/prepare-space.sh — this file is
         installed only by the golden build).
      3. The user is the Space's single automation user.

    Set CUA_APPROVER_I_UNDERSTAND=1 only for deliberate testing inside a guest
    whose marker has been sanitized away. It does NOT bypass the VM check.
    """
    model = ""
    try:
        model = subprocess.run(
            ["/usr/sbin/sysctl", "-n", "hw.model"],
            capture_output=True, text=True, timeout=10,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        pass
    if not ("VirtualMac" in model or "Apple Virtual Machine" in model):
        sys.exit(
            f"REFUSING TO RUN: hw.model={model!r} is not an Apple Virtual Machine.\n"
            "permission-approver auto-approves permission prompts and is safe ONLY\n"
            "inside a disposable single-user Space VM. On a host it would hand any\n"
            "process that can raise a TCC prompt the permission it asked for."
        )

    marker = os.path.expanduser("~/.cua/prepare-space.sh")
    if not os.path.exists(marker) and os.environ.get("CUA_APPROVER_I_UNDERSTAND") != "1":
        sys.exit(
            f"REFUSING TO RUN: Space marker {marker} is absent — this VM was not\n"
            "provisioned as a Cua Space. Set CUA_APPROVER_I_UNDERSTAND=1 to override\n"
            "inside a guest you are deliberately testing."
        )

    user = os.environ.get("USER") or ""
    expected = os.environ.get("CUA_SPACE_USER", "lume")
    if user != expected and os.environ.get("CUA_APPROVER_I_UNDERSTAND") != "1":
        sys.exit(
            f"REFUSING TO RUN: running as {user!r}, not the Space user {expected!r}."
        )

    log(f"host guard passed (hw.model={model}, user={user})")


# ---------------------------------------------------------------------------
# MCP client
# ---------------------------------------------------------------------------
class DriverMCP:
    """One persistent stdio session with cua-driver's MCP.

    PERSISTENT ON PURPOSE. cua-driver scopes its element-index cache to the
    TRANSPORT SESSION, which for stdio is the MCP process. A get_window_state in
    one session and a click in the next would address an index map that no
    longer exists. So the snapshot and the press must ride the same process.
    """

    def __init__(self) -> None:
        self.proc: subprocess.Popen | None = None
        self.next_id = 1

    @property
    def conn(self):  # kept for the main loop's "connected?" check
        return self.proc

    def connect(self) -> None:
        self.close()
        self.proc = subprocess.Popen(
            [DRIVER_BIN, "mcp", "--socket", DRIVER_SOCKET],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, bufsize=1,
        )
        # MCP requires the initialize handshake before tools/call.
        self.call_raw("initialize", {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "cua-permission-approver", "version": "1"},
        })
        self.notify("notifications/initialized")

    def close(self) -> None:
        if self.proc is not None:
            try:
                self.proc.kill()
                self.proc.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                pass
            self.proc = None

    def _send(self, body: dict) -> None:
        assert self.proc is not None and self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(body) + "\n")
        self.proc.stdin.flush()

    def _post(self, body: dict) -> dict | None:
        self._send(body)
        if "id" not in body:
            return None
        assert self.proc is not None and self.proc.stdout is not None
        # Bounded: skip at most 64 unrelated lines (server notifications).
        for _ in range(64):
            line = self.proc.stdout.readline()
            if not line:
                raise OSError("cua-driver MCP exited")
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if msg.get("id") == body["id"]:
                return msg
        raise OSError(f"no response to {body.get('method')}")

    def call_raw(self, method: str, params: dict) -> dict:
        rid = self.next_id
        self.next_id += 1
        out = self._post({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        if out is None:
            raise OSError(f"empty response to {method}")
        if "error" in out:
            raise OSError(f"{method}: {out['error']}")
        return out.get("result", {})

    def notify(self, method: str) -> None:
        try:
            self._post({"jsonrpc": "2.0", "method": method, "params": {}})
        except (OSError, ValueError):
            pass

    def tool(self, name: str, args: dict) -> dict:
        result = self.call_raw("tools/call", {"name": name, "arguments": args})
        if result.get("isError"):
            texts = [c.get("text", "") for c in result.get("content", [])]
            raise OSError(f"{name} failed: {' '.join(texts)[:300]}")
        structured = result.get("structuredContent")
        if structured is not None:
            return structured
        # Fall back to parsing the first text content as JSON.
        for c in result.get("content", []):
            if c.get("type") == "text":
                try:
                    return json.loads(c["text"])
                except ValueError:
                    return {"text": c["text"]}
        return {}


# ---------------------------------------------------------------------------
# Dialog handling
# ---------------------------------------------------------------------------
# NOTE ON WHERE THE TEXT IS.
# get_window_state's structured `elements` array contains ONLY ACTIONABLE nodes
# (build_elements_array_with_token filters on `element_index.is_some()`), so the
# AXStaticText that carries "…would like to access the microphone" is NOT in it.
# The full tree, static text included, is in `tree_markdown`. So the phrase gate
# and the human summary read the markdown; the buttons come from `elements`.
# Getting this backwards makes the approver match nothing at all.
def element_text(el: dict) -> str:
    parts = [el.get("label"), el.get("value"), el.get("value_description")]
    return " ".join(p for p in parts if isinstance(p, str) and p).strip()


def choose_button(elements: list[dict]) -> tuple[dict | None, str]:
    """Pick the button to press. Returns (element, reason)."""
    buttons: dict[str, dict] = {}
    for el in elements:
        role = (el.get("role") or "").lower()
        if role not in ("axbutton", "button"):
            continue
        if not el.get("element_token"):
            continue
        label = (el.get("label") or el.get("title") or el.get("value") or "").strip()
        key = label.lower().replace("’", "'")
        if key and key not in buttons:
            buttons[key] = el
    if not buttons:
        return None, "no AXButton with an element_token in the dialog"
    for want in APPROVE_BUTTONS:
        if want in buttons:
            return buttons[want], want
    offered = ", ".join(sorted(buttons))
    only_denials = all(b in DENY_BUTTONS for b in buttons)
    if only_denials:
        return None, f"dialog offers only denial buttons ({offered}) — LEFT ALONE deliberately"
    return None, f"no approving button among: {offered}"


KEYCHAIN_PANEL_RE = re.compile(r"wants to use the .*keychain", re.I)
BLOCKED_MARKER = os.path.expanduser("~/.cua/SPACE-BLOCKED-credential-panel.txt")


def write_blocked_marker(app: str, title: str, summary: str) -> None:
    """Leave a file an agent can find without reading this log.

    A stalled Space looks identical to a slow one from inside. An agent that
    checks its working directory or home for this marker learns the difference
    immediately.
    """
    try:
        os.makedirs(os.path.dirname(BLOCKED_MARKER), exist_ok=True)
        with open(BLOCKED_MARKER, "w", encoding="utf-8") as fh:
            fh.write(
                "THIS SPACE IS BLOCKED BY A CREDENTIAL PANEL.\n\n"
                f"process: {app}\ntitle:   {title}\ntext:    {summary}\n\n"
                "It asks for a password this Space does not know, so the "
                "permission approver could not answer it. Nothing on the "
                "desktop will progress until it is gone. Do not keep waiting.\n"
            )
    except OSError:
        pass


def answer_credential_panel(
    mcp: "DriverMCP", app: str, pid: int, wid: int, title: str,
    elements: list[dict], summary: str,
) -> bool:
    """Answer a keychain password panel using the password this Space set.

    Returns True when handled. Only keychain panels are answered — anything
    asking for a different secret is left alone and reported.
    """
    if not KEYCHAIN_PANEL_RE.search(summary or "") and not KEYCHAIN_PANEL_RE.search(title or ""):
        return False

    field = next(
        (e for e in elements
         if (e.get("role") or "").lower() in ("axtextfield", "textfield", "axsecuretextfield")
         and e.get("element_token")),
        None,
    )
    btn, reason = choose_button(elements)
    if field is None or btn is None:
        log(f"KEYCHAIN PANEL not answerable: field={field is not None} button={reason}")
        return False

    # The account password. The golden build re-keys the login keychain to it
    # (sanitize-golden.sh), so it is the right answer for any keychain panel a
    # Space can legitimately raise.
    pw = "lume"
    target = {"pid": pid, "window_id": wid, "delivery_mode": "background"}
    try:
        # A password box is usually an AXSecureTextField, and those commonly
        # refuse an AXValue write — the call "succeeds" and the field stays
        # empty. So try the write, then fall back to focusing the field and
        # typing, which goes through the same path a person would.
        try:
            mcp.tool("set_value",
                     {**target, "element_token": field["element_token"], "value": pw})
        except OSError:
            mcp.tool("click", {**target, "element_token": field["element_token"]})
            mcp.tool("type_text", {**target, "text": pw})
        mcp.tool("click", {**target, "element_token": btn["element_token"]})
    except OSError as exc:
        log(f"KEYCHAIN PANEL answer FAILED: app={app!r} text={summary!r} — {exc}")
        return False

    log(f"KEYCHAIN PANEL ANSWERED: app={app!r} button={btn.get('label')!r} text={summary!r}")
    try:
        os.remove(BLOCKED_MARKER)
    except OSError:
        pass
    return True


def is_permission_dialog(
    app_name: str, title: str, elements: list[dict], tree_markdown: str
) -> tuple[bool, str]:
    if app_name in PROMPT_PROCESSES:
        return True, f"drawn by prompt process {app_name}"
    blob = " ".join([title, tree_markdown] + [element_text(el) for el in elements])
    m = PERMISSION_RE.search(blob)
    if m:
        return True, f"text matched {m.group(0)!r}"
    return False, ""


def dialog_summary(title: str, tree_markdown: str) -> str:
    """The dialog's own words, for the log, out of the markdown tree."""
    texts = []
    for line in tree_markdown.splitlines():
        line = line.strip(" -*#\t")
        if not line:
            continue
        if "AXStaticText" in line or "StaticText" in line:
            # Rows render roughly as: AXStaticText "the message"
            quoted = re.findall(r'"([^"]{4,})"', line)
            texts.extend(quoted)
    body = " / ".join(texts[:4]) or title
    return body[:300]


def quartz_windows() -> list[dict]:
    """On-screen windows on EVERY layer, via CoreGraphics. No permission needed."""
    opts = (
        Quartz.kCGWindowListOptionOnScreenOnly
        | Quartz.kCGWindowListExcludeDesktopElements
    )
    out = []
    for w in Quartz.CGWindowListCopyWindowInfo(opts, Quartz.kCGNullWindowID) or []:
        bounds = w.get("kCGWindowBounds") or {}
        out.append({
            "window_id": w.get("kCGWindowNumber"),
            "pid": w.get("kCGWindowOwnerPID"),
            "app_name": w.get("kCGWindowOwnerName") or "",
            # kCGWindowName needs Screen Recording; absent is fine, the AX tree
            # supplies the text.
            "title": w.get("kCGWindowName") or "",
            "layer": w.get("kCGWindowLayer"),
            "bounds": {
                "x": bounds.get("X", 0), "y": bounds.get("Y", 0),
                "width": bounds.get("Width", 0), "height": bounds.get("Height", 0),
            },
        })
    return out


def discover_windows(mcp: DriverMCP) -> list[dict]:
    if HAVE_QUARTZ:
        try:
            return quartz_windows()
        except Exception as exc:  # noqa: BLE001 - discovery must never be fatal
            log(f"Quartz enumeration failed ({exc}); falling back to list_windows")
    result = mcp.tool("list_windows", {"on_screen_only": True})
    if isinstance(result, list):
        return result
    return result.get("windows") or []


def main() -> int:
    require_disposable_space()
    discovery = "CGWindowList (all layers)" if HAVE_QUARTZ else \
        "cua-driver list_windows (LAYER 0 ONLY — a non-layer-0 alert will be MISSED)"
    log(f"permission-approver starting (MCP {DRIVER_BIN} mcp, poll {POLL_SECONDS}s, "
        f"discovery: {discovery})")
    if not HAVE_QUARTZ:
        log("WARNING: Quartz is not importable by this interpreter. Run the approver "
            "with ~/.cua-server/venv/bin/python (it has pyobjc-framework-Quartz) so "
            "discovery covers every window layer.")
    mcp = DriverMCP()
    # window_id -> attempts; and a set of ids we have given up on / already logged
    attempts: dict[int, int] = {}
    not_a_dialog: set[int] = set()
    reported: set[tuple[int, str]] = set()
    approved_count = 0

    while True:
        try:
            if mcp.conn is None:
                mcp.connect()
                log("connected to cua-driver MCP")

            records = discover_windows(mcp)

            live_ids = set()
            for w in records:
                wid = w.get("window_id")
                pid = w.get("pid")
                app = (w.get("app_name") or "").strip()
                title = (w.get("title") or "").strip()
                if wid is None or pid is None:
                    continue
                live_ids.add(wid)


                if attempts.get(wid, 0) >= MAX_ATTEMPTS:
                    continue

                # NEGATIVE CACHE. An alert is always a NEW window, so a
                # window_id already inspected and found not to be a permission
                # dialog can never become one. Without this the approver walks
                # the AX tree of every window on the desktop twice a second,
                # which on a loaded Unity Editor is real CPU stolen from the run
                # it is supposed to be unblocking.
                if wid in not_a_dialog:
                    continue

                # Cheap pre-filter. Permission alerts are small. A window that
                # is not from a prompt-drawing process and is too big to be an
                # alert is skipped without an AX read. The size gate is generous
                # because the local-network alert is an untitled panel of the
                # ASKING app, not of UserNotificationCenter, so app name alone
                # is not a sufficient filter.
                if app not in PROMPT_PROCESSES:
                    b = w.get("bounds") or {}
                    if (b.get("width") or 0) > 900 or (b.get("height") or 0) > 700:
                        not_a_dialog.add(wid)
                        continue

                try:
                    state = mcp.tool("get_window_state", {
                        "pid": pid, "window_id": wid, "include_screenshot": False,
                        # An alert has a handful of elements. Anything with a
                        # big tree is not one, and bounding the walk keeps this
                        # cheap on Electron/Unity windows that slip the size
                        # gate.
                        "max_elements": 200,
                    })
                except OSError:
                    # window_id_not_found, or a window owned by a process we
                    # cannot read. Either way it is not actionable.
                    not_a_dialog.add(wid)
                    continue
                elements = state.get("elements") or []
                tree_md = state.get("tree_markdown") or ""
                if not elements:
                    # No actionable element: nothing to press now, and an alert
                    # always has buttons. Do NOT negative-cache — a window can
                    # be caught mid-construction.
                    continue

                # Credential panels first: they are drawn by SecurityAgent and
                # are NOT phrased like consent prompts, so is_permission_dialog
                # would reject them. This runs here, and not earlier in the loop,
                # because it needs `elements` and the summary text — reading them
                # before get_window_state is what made an earlier revision of
                # this file crash on every SecurityAgent window.
                if app in CREDENTIAL_PROCESSES:
                    cred_summary = dialog_summary(title, tree_md)
                    if answer_credential_panel(
                        mcp, app, pid, wid, title, elements, cred_summary
                    ):
                        continue
                    key = (wid, "credential")
                    if key not in reported:
                        reported.add(key)
                        log(
                            f"!! UNANSWERED CREDENTIAL PANEL — THE SPACE IS BLOCKED !! "
                            f"app={app!r} title={title!r} text={cred_summary!r}. "
                            "It asks for a password this Space does not know, so it "
                            "cannot be answered from here. Nothing will proceed until "
                            "it is gone. If you are an agent driving this Space: this "
                            "is why your work appears stalled — surface it, do not wait."
                        )
                        write_blocked_marker(app, title, cred_summary)
                    continue

                ok, why = is_permission_dialog(app, title, elements, tree_md)
                if not ok:
                    not_a_dialog.add(wid)
                    continue

                summary = dialog_summary(title, tree_md)
                btn, reason = choose_button(elements)
                if btn is None:
                    key = (wid, reason)
                    if key not in reported:
                        reported.add(key)
                        log(f"PERMISSION DIALOG NOT APPROVED: app={app!r} pid={pid} "
                            f"text={summary!r} — {reason}")
                    attempts[wid] = attempts.get(wid, 0) + 1
                    continue

                attempts[wid] = attempts.get(wid, 0) + 1
                label = btn.get("label") or reason
                # element_token is the only element target cua-driver accepts:
                # it carries its own validity and fails loudly if a newer
                # snapshot superseded it, rather than pressing whatever now
                # sits at index N.
                click_args = {"pid": pid, "window_id": wid, "delivery_mode": "background",
                              "element_token": btn["element_token"]}
                try:
                    mcp.tool("click", click_args)
                except OSError as exc:
                    log(f"APPROVAL FAILED (attempt {attempts[wid]}/{MAX_ATTEMPTS}): "
                        f"app={app!r} pid={pid} button={label!r} text={summary!r} — {exc}")
                    if attempts[wid] >= MAX_ATTEMPTS:
                        log(f"GIVING UP on window {wid} ({app!r}) after {MAX_ATTEMPTS} "
                            "attempts — this dialog is not drivable from the AX API.")
                    continue

                approved_count += 1
                log(f"AUTO-APPROVED #{approved_count}: app={app!r} pid={pid} "
                    f"window={wid} matched({why}) button={label!r} text={summary!r}")

            # Forget windows that are gone, so a recycled id starts fresh.
            for wid in list(attempts):
                if wid not in live_ids:
                    del attempts[wid]
            not_a_dialog &= live_ids
            reported = {k for k in reported if k[0] in live_ids}

        except KeyboardInterrupt:
            raise
        except Exception as exc:  # noqa: BLE001 - see below
            # Deliberately broad. This is a safety net: if it dies, a Space can
            # sit behind a dialog for the whole run with nothing watching, and
            # because launchd restarts it the log fills with healthy-looking
            # "starting/connected" banners while it covers nothing. That is
            # exactly what happened when a bug in the credential-panel path
            # raised UnboundLocalError on every SecurityAgent window -- not in
            # this tuple, so it killed the process, every 10s, silently.
            #
            # A cycle that fails for ANY reason should log and keep polling.
            log(f"cycle error ({type(exc).__name__}): {exc}")
            mcp.close()
            time.sleep(3)
            continue
        except KeyboardInterrupt:
            log("interrupted")
            return 0

        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    sys.exit(main())
