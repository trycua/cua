"""Vercel's agent-desktop as an AgentEnv computer driver on macOS.

agent-desktop (https://github.com/vercel-labs/agent-desktop, Apache-2.0) is a CLI with no MCP server: it reads an app's
accessibility tree, prints it with a ref for each element, and acts on a ref. This driver wraps its snapshot and action
commands as tools, one tool for each command, and runs each in the VM's GUI session as its user. Agents get:

* its tools (``tools.json`` pins them): ``snapshot``, ``find``, ``click``, ``type`` and the rest;
* agent-desktop's own skill, as the pinned binary embeds it (``agent-desktop skills get desktop``), as the instructions;
* the neutral ``computer_*`` tools of ``macos_computer.py``, which an env hides with its ``[tools]`` list.

Pinned: agent-desktop 1.0.2, ``aarch64-apple-darwin``. Its ready checks fail clearly when the executable isn't the
pinned one, when a wrapped command is missing from its ``--help``, or when macOS hasn't granted Accessibility and
Screen Recording to it.

Run ``python driver.py --write-manifest`` to rewrite ``tools.json`` after changing ``TOOLS``.

Needs ``agentenv-framework-protocol[computer]`` where it runs, and the executable at ``EXECUTABLE``.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from agentenv_protocol import EnvironmentTool, ForwardedTool, server_instructions
from agentenv_protocol.computer import ReadyCheck, computer_driver
from mcp.types import CallToolResult, ImageContent, TextContent

sys.path.insert(0, str(Path(__file__).parent))
from macos_computer import MacComputerDriver  # noqa: E402

VERSION = "1.0.2"
ARCHIVE = f"agent-desktop-v{VERSION}-aarch64-apple-darwin.tar.gz"
ARCHIVE_SHA256 = "7bee385196d7b478de80cf05ca4304aaa38f0eba593b3406ad3c247c1773be19"
ARCHIVE_URL = f"https://github.com/vercel-labs/agent-desktop/releases/download/v{VERSION}/{ARCHIVE}"
#: sha256 of ``agent-desktop`` and of ``agent-desktop-macos-helper`` in that archive.
EXECUTABLE_SHA256 = "bf0a3c9256512f2f57288d37ccd17d7f03665fccc53c2f74011a49b98e61ba59"
HELPER_SHA256 = "522829116cb006b5770c6a82b361f466a4775f794d6af4083e75231a2d6bb551"
EXECUTABLE = Path(
    os.environ.get("AGENT_DESKTOP_BIN", "/opt/agentenv/tools/agent-desktop/agent-desktop")
)
MANIFEST = Path(__file__).with_name("tools.json")
#: The skill texts the binary embeds, and the references of the core one.
SKILLS = ("desktop", "platform")
REFERENCES = ("commands-observation", "commands-interaction", "commands-system", "workflows")
DEFAULT_TIMEOUT_S = 60.0
PERMISSIONS = ("accessibility", "screen_recording")


@dataclass(frozen=True)
class Param:
    """One argument of a tool: how it reaches the command line."""

    name: str
    type: str = "string"  # string, integer, boolean, array (a repeatable flag), or an enum list
    description: str = ""
    flag: str | None = None  # None: a positional; else the flag it is, "--x"
    enum: tuple[str, ...] = ()
    required: bool = False


@dataclass(frozen=True)
class Tool:
    name: str
    command: str
    description: str
    params: tuple[Param, ...] = ()
    #: Physical input: the command refuses without ``--headed``, so the tool always passes it.
    headed: bool = False
    #: ``screenshot`` writes a PNG to a path, which the tool returns as an image.
    image: bool = False
    extra: dict = field(default_factory=dict)

    def schema(self) -> dict:
        properties = {}
        for p in self.params:
            entry: dict[str, Any] = (
                {"type": "array", "items": {"type": "string"}}
                if p.type == "array"
                else {"type": p.type}
            )
            if p.enum:
                entry["enum"] = list(p.enum)
            if p.description:
                entry["description"] = p.description
            properties[p.name] = entry
        schema: dict[str, Any] = {
            "type": "object",
            "properties": properties,
            "additionalProperties": False,
        }
        if required := [p.name for p in self.params if p.required]:
            schema["required"] = required
        return schema


def _ref(
    desc: str = "The element's qualified ref from a snapshot, as printed: @<snapshot_id>:e<N>.",
) -> Param:
    return Param("ref", description=desc, required=True)


#: What a ref action takes beside its ref: a wait for the UI to settle, and how long to wait for the ref.
def _act(*extra: Param) -> tuple[Param, ...]:
    return (
        _ref(),
        *extra,
        Param(
            "timeout_ms",
            "integer",
            "How long to wait for the ref to resolve, in milliseconds (default 5000).",
            "--timeout-ms",
        ),
        Param(
            "wait_for",
            description="After the action, poll until an element matching role:text appears, then return its snapshot.",
            flag="--wait-for",
        ),
        Param(
            "wait_for_gone",
            description="After the action, poll until no element matches role:text.",
            flag="--wait-for-gone",
        ),
    )


_APP = Param("app", description="Application name.", flag="--app")
_WINDOW_ID = Param("window_id", description="A window ID from list_windows.", flag="--window-id")
_PROP_GET = ("text", "value", "title", "bounds", "role", "states")
_PROP_IS = ("visible", "enabled", "checked", "focused", "expanded", "selected")
_SURFACES = ("window", "focused", "menu", "menubar", "sheet", "popover", "alert")

TOOLS: tuple[Tool, ...] = (
    Tool(
        "snapshot",
        "snapshot",
        "Capture an app's accessibility tree as JSON, with a ref for each element you can act on.",
        (
            _APP,
            _WINDOW_ID,
            Param(
                "interactive_only",
                "boolean",
                "Only the elements you can act on.",
                "--interactive-only",
            ),
            Param("compact", "boolean", "Collapse single-child unnamed nodes.", "--compact"),
            Param(
                "skeleton",
                "boolean",
                "A shallow map, with children_count on cut branches.",
                "--skeleton",
            ),
            Param("max_depth", "integer", "Maximum tree depth (default 10).", "--max-depth"),
            Param(
                "include_bounds", "boolean", "Include each element's bounds.", "--include-bounds"
            ),
            Param(
                "surface",
                description="The surface to snapshot (default window).",
                flag="--surface",
                enum=_SURFACES,
            ),
            Param(
                "root",
                description="Start from this ref's subtree instead of the window.",
                flag="--root",
            ),
            Param(
                "wait_for",
                description="Poll until an element matching role:text appears.",
                flag="--wait-for",
            ),
            Param(
                "timeout_ms",
                "integer",
                "Observation deadline in milliseconds (default 3000).",
                "--timeout-ms",
            ),
        ),
    ),
    Tool(
        "find",
        "find",
        "Search an app's elements by role, name, value or text.",
        (
            _APP,
            _WINDOW_ID,
            Param(
                "role",
                description="An accessibility role: button, textfield, checkbox...",
                flag="--role",
            ),
            Param("name", description="The accessible name or label.", flag="--name"),
            Param("value", description="The current value.", flag="--value"),
            Param(
                "text", description="Text in the name, value, title or description.", flag="--text"
            ),
            Param("description", description="The accessible description.", flag="--description"),
            Param(
                "native_id",
                description="The native automation id (AXIdentifier).",
                flag="--native-id",
            ),
            Param(
                "exact",
                "boolean",
                "Require exact, case-insensitive name, description and value matches.",
                "--exact",
            ),
            Param("root", description="Search only this ref's subtree.", flag="--root"),
            Param(
                "surface", description="The surface to search.", flag="--surface", enum=_SURFACES
            ),
            Param("state", "array", "State filters, each TOKEN or TOKEN=true|false.", "--state"),
            Param("count", "boolean", "Return the match count only.", "--count"),
            Param(
                "timeout_ms",
                "integer",
                "Traversal deadline in milliseconds (default 5000).",
                "--timeout-ms",
            ),
        ),
    ),
    Tool(
        "get",
        "get",
        "Read one property of an element.",
        (
            _ref(),
            Param(
                "property",
                description="The property to read (default text).",
                flag="--property",
                enum=_PROP_GET,
            ),
        ),
    ),
    Tool(
        "is",
        "is",
        "Check one state of an element.",
        (
            _ref(),
            Param(
                "property",
                description="The state to check (default visible).",
                flag="--property",
                enum=_PROP_IS,
            ),
        ),
    ),
    Tool(
        "screenshot",
        "screenshot",
        "Take a PNG screenshot of an app's window.",
        (_APP, _WINDOW_ID),
        image=True,
    ),
    Tool("list_apps", "list-apps", "List the running apps.", (_APP,)),
    Tool(
        "list_windows",
        "list-windows",
        "List the open windows, with the window IDs other tools take.",
        (_APP,),
    ),
    Tool("click", "click", "Click an element through its accessibility press action.", _act()),
    Tool(
        "double_click",
        "double-click",
        "Double-click an element with the real pointer.",
        _act(),
        headed=True,
    ),
    Tool(
        "triple_click",
        "triple-click",
        "Triple-click an element with the real pointer.",
        _act(),
        headed=True,
    ),
    Tool("right_click", "right-click", "Open an element's context menu.", _act()),
    Tool(
        "type",
        "type",
        "Insert text into a text element.",
        _act(Param("text", description="The text to type.", required=True)),
    ),
    Tool(
        "set_value",
        "set-value",
        "Set an element's value directly.",
        _act(Param("value", description="The value to set.", required=True)),
    ),
    Tool("clear", "clear", "Clear an element's value.", _act()),
    Tool("focus", "focus", "Set keyboard focus on an element.", _act()),
    Tool(
        "select",
        "select",
        "Select an option in a list or dropdown.",
        _act(Param("value", description="The option to select.", required=True)),
    ),
    Tool("toggle", "toggle", "Toggle a checkbox or switch.", _act()),
    Tool("check", "check", "Set a checkbox or switch to checked (idempotent).", _act()),
    Tool("uncheck", "uncheck", "Set a checkbox or switch to unchecked (idempotent).", _act()),
    Tool("expand", "expand", "Expand a disclosure element.", _act()),
    Tool("collapse", "collapse", "Collapse a disclosure element.", _act()),
    Tool(
        "scroll",
        "scroll",
        "Scroll an element.",
        _act(
            Param(
                "direction",
                description="up, down, left or right (default down).",
                flag="--direction",
                enum=("up", "down", "left", "right"),
            ),
            Param("amount", "integer", "Scroll units (default 3).", "--amount"),
        ),
    ),
    Tool("scroll_to", "scroll-to", "Scroll an element into view.", _act()),
    Tool(
        "press",
        "press",
        "Press a key combo in an app: return, escape, cmd+c, shift+tab...",
        (
            Param("combo", description="The key combo.", required=True),
            _WINDOW_ID,
        ),
    ),
    Tool(
        "hover",
        "hover",
        "Move the real pointer over an element.",
        (Param("ref", description="The element's qualified ref."),),
        headed=True,
    ),
    Tool(
        "drag",
        "drag",
        "Drag from one element or point to another with the real pointer.",
        (
            Param("from_ref", description="The source ref.", flag="--from"),
            Param("to_ref", description="The destination ref.", flag="--to"),
            Param("from_xy", description="The source as x,y.", flag="--from-xy"),
            Param("to_xy", description="The destination as x,y.", flag="--to-xy"),
            Param("duration", "integer", "The drag's duration in milliseconds.", "--duration"),
            Param(
                "drop_delay",
                "integer",
                "Hold over the destination this many ms before releasing (default 500).",
                "--drop-delay",
            ),
        ),
        headed=True,
    ),
    Tool(
        "launch",
        "launch",
        "Launch an app, or attach to the running one.",
        (
            Param("app", description="The application name or bundle ID.", required=True),
            Param("args", "array", "Command-line arguments for the app.", "--arg"),
            Param("cwd", description="The working directory.", flag="--cwd"),
            Param(
                "activate", "boolean", "Bring the app forward and wait for a window.", "--activate"
            ),
            Param(
                "timeout",
                "integer",
                "Upper bound for the whole launch, in ms (default 30000).",
                "--timeout",
            ),
        ),
    ),
    Tool(
        "close_app",
        "close-app",
        "Quit an app.",
        (
            Param("app", description="The application name.", required=True),
            Param("force", "boolean", "Terminate its processes instead of quitting.", "--force"),
        ),
    ),
    Tool(
        "focus_window",
        "focus-window",
        "Bring a window forward.",
        (
            _WINDOW_ID,
            _APP,
            Param("title", description="Part of the window's title.", flag="--title"),
        ),
    ),
    Tool(
        "clipboard_get",
        "clipboard-get",
        "Read the clipboard.",
        (
            Param(
                "format",
                description="text (default), auto, image or file-urls.",
                flag="--format",
                enum=("text", "auto", "image", "file-urls"),
            ),
        ),
    ),
    Tool(
        "clipboard_set",
        "clipboard-set",
        "Write text to the clipboard.",
        (Param("text", description="The text.", required=True),),
    ),
    Tool(
        "wait",
        "wait",
        "Pause, or block until an element, window, text or menu appears.",
        (
            Param("ms", "integer", "Milliseconds to pause."),
            Param("element", description="Block until this element ref appears.", flag="--element"),
            Param(
                "window",
                description="Block until a window with this title appears.",
                flag="--window",
            ),
            Param(
                "text",
                description="Block until this text appears in the app's tree.",
                flag="--text",
            ),
            Param("menu", "boolean", "Block until a menu is open.", "--menu"),
            Param("menu_closed", "boolean", "Block until the menu is dismissed.", "--menu-closed"),
            Param("app", description="Scope the wait to this app.", flag="--app"),
            Param("timeout", "integer", "Timeout in milliseconds (default 30000).", "--timeout"),
        ),
    ),
)
BY_NAME = {t.name: t for t in TOOLS}


def argv_for(tool: Tool, arguments: dict[str, Any]) -> list[str]:
    """The command line of one call: the command, its flags, then ``--`` and its positionals, so a value that starts
    with a dash is never taken for a flag."""
    unknown = sorted(set(arguments) - {p.name for p in tool.params})
    if unknown:
        raise ValueError(f"{tool.name} takes no argument named {', '.join(unknown)}")
    if missing := [p.name for p in tool.params if p.required and p.name not in arguments]:
        raise ValueError(f"{tool.name} needs {', '.join(missing)}")
    flags: list[str] = [tool.command]
    positionals: list[str] = []
    for p in tool.params:
        if p.name not in arguments or arguments[p.name] is None:
            continue
        value = arguments[p.name]
        if p.flag is None:
            positionals.append(str(value))
        elif p.type == "boolean":
            if value:
                flags.append(p.flag)
        elif p.type == "array":
            flags.extend(f"{p.flag}={item}" for item in value)
        else:
            flags.append(f"{p.flag}={value}")
    if tool.headed:
        flags.append("--headed")
    return [*flags, *(["--", *positionals] if positionals else [])]


def manifest() -> dict:
    """What ``tools.json`` pins: the version, and each tool's name, command and input schema."""
    return {
        "version": VERSION,
        "tools": [
            {
                "name": t.name,
                "command": t.command,
                "description": t.description,
                "inputSchema": t.schema(),
            }
            for t in TOOLS
        ],
    }


def commands_check(help_text: str) -> ReadyCheck:
    """Whether the running binary still lists every command the tools wrap."""
    listed = set(re.findall(r"^  ([a-z][a-z-]*) ", help_text, flags=re.MULTILINE))
    missing = sorted({t.command for t in TOOLS} - listed)
    return ReadyCheck(
        "agent-desktop commands",
        not missing,
        f"{len({t.command for t in TOOLS})} wrapped commands present"
        if not missing
        else f"missing from --help: {', '.join(missing)}",
    )


def permissions_check(status: dict) -> ReadyCheck:
    """Ready when ``agent-desktop status`` reports Accessibility and Screen Recording granted."""
    permissions = ((status.get("data") or {}).get("permissions")) or {}
    missing = [n for n in PERMISSIONS if (permissions.get(n) or {}).get("state") != "granted"]
    return ReadyCheck(
        "macOS permissions",
        not missing,
        "Accessibility and Screen Recording granted"
        if not missing
        else f"not granted to agent-desktop: {', '.join(missing)} (the image's TCC rows don't match this binary)",
    )


def envelope_result(code: int, out: str, err: str) -> CallToolResult:
    """The tool result of one command: its JSON envelope as text, an error when ``ok`` is false or it exited non-zero."""
    try:
        failed = not json.loads(out).get("ok", code == 0)
    except ValueError:
        failed = code != 0
    text = out.strip() or err.strip() or f"agent-desktop exited {code}"
    return CallToolResult(
        isError=failed or code != 0, content=[TextContent(type="text", text=text)]
    )


@computer_driver(name="agent-desktop", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
class AgentDesktopMacOS(MacComputerDriver):
    def __init__(self) -> None:
        super().__init__()
        self._pinned: list[ReadyCheck] | None = None
        self._skill: tuple[str, str, dict[str, str]] | None = None

    def forwarded_tools(self) -> list[ForwardedTool]:
        pinned = (
            json.loads(MANIFEST.read_text())["tools"] if MANIFEST.exists() else manifest()["tools"]
        )
        return [
            ForwardedTool(
                EnvironmentTool(
                    name=t["name"], description=t["description"], inputSchema=t["inputSchema"]
                ),
                self._forward(BY_NAME[t["name"]]),
            )
            for t in pinned
        ]

    def _forward(self, tool: Tool):
        async def call(**arguments: Any) -> CallToolResult:
            try:
                argv = argv_for(tool, arguments)
            except ValueError as e:
                return CallToolResult(isError=True, content=[TextContent(type="text", text=str(e))])
            if tool.image:
                return await self._screenshot(argv)
            code, out, err = await self.run_cli(argv, timeout_s=_timeout(arguments))
            return envelope_result(code, out, err)

        return call

    async def _screenshot(self, argv: list[str]) -> CallToolResult:
        with tempfile.TemporaryDirectory(prefix="agent-desktop-shot-") as folder:
            path = Path(folder) / "shot.png"
            code, out, err = await self.run_cli(
                [*argv, "--", str(path)], timeout_s=DEFAULT_TIMEOUT_S
            )
            result = envelope_result(code, out, err)
            if not result.isError and path.exists():
                result.content.append(
                    ImageContent(
                        type="image",
                        data=base64.b64encode(path.read_bytes()).decode(),
                        mimeType="image/png",
                    )
                )
            return result

    async def run_cli(self, argv: list[str], *, timeout_s: float) -> tuple[int, str, str]:
        """Run agent-desktop in the session, as its user, with its output."""
        try:
            proc = await asyncio.create_subprocess_exec(
                str(EXECUTABLE),
                *argv,
                stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=_env(),
            )
        except OSError as e:
            return 127, "", f"agent-desktop can't start: {e}"
        try:
            out, err = await asyncio.wait_for(proc.communicate(), timeout_s)
        except TimeoutError:
            proc.kill()
            await proc.wait()
            return 124, "", f"agent-desktop {argv[0]} didn't finish within {timeout_s:.0f} s"
        return proc.returncode or 0, out.decode(errors="replace"), err.decode(errors="replace")

    @server_instructions
    def agent_desktop_instructions(self) -> str:
        return (
            f"agent-desktop {VERSION}: each tool here runs the agent-desktop command of the same name (`click` runs "
            "`agent-desktop click`), and its arguments are that command's flags and arguments. Where the guide that "
            "follows shows a shell command, call the tool instead. Every call returns the command's JSON envelope."
        )

    async def ready_checks(self) -> list[ReadyCheck]:
        """The pinned executable and its commands, checked until they pass once, and the TCC grants, checked every
        time: a reset can lose them, and then every action fails silently."""
        if self._pinned is None or not all(c.ok for c in self._pinned):
            self._pinned = await self._pinned_checks()
        code, out, _ = await self.run_cli(["status"], timeout_s=30)
        try:
            status = json.loads(out)
        except ValueError:
            status = {}
        return [*self._pinned, permissions_check(status)]

    async def _pinned_checks(self) -> list[ReadyCheck]:
        if not EXECUTABLE.is_file():
            return [ReadyCheck("agent-desktop", False, f"{EXECUTABLE} isn't installed")]
        digest = await asyncio.to_thread(
            lambda: hashlib.sha256(EXECUTABLE.read_bytes()).hexdigest()
        )
        code, out, _ = await self.run_cli(["version"], timeout_s=30)
        try:
            reported = json.loads(out)["data"]["version"]
        except (ValueError, KeyError, TypeError):
            reported = None
        pinned = digest == EXECUTABLE_SHA256 and reported == VERSION
        _, help_text, _ = await self.run_cli(["--help"], timeout_s=30)
        return [
            ReadyCheck(
                "agent-desktop",
                pinned,
                f"agent-desktop {VERSION}, the pinned executable"
                if pinned
                else f"sha256 {digest} and version {reported}, not the pinned {EXECUTABLE_SHA256} and {VERSION}",
            ),
            commands_check(help_text),
        ]

    async def bundled_skill(self) -> tuple[str, str, dict[str, str]] | None:
        if self._skill is None:
            self._skill = await self._read_skill()
        return self._skill

    async def _read_skill(self) -> tuple[str, str, dict[str, str]]:
        """agent-desktop's skill as the binary embeds it: its core skill leads, the platform skill and the core
        skill's references come beside it."""

        async def get(*words: str) -> str:
            code, out, err = await self.run_cli(["skills", "get", *words], timeout_s=30)
            if code:
                raise RuntimeError(
                    f"agent-desktop skills get {' '.join(words)}: {err.strip() or out.strip()}"
                )
            return json.loads(out)["data"]["content"]

        core = await get("desktop")
        files = {"platform.md": await get("platform")}
        for reference in REFERENCES:
            files[f"{reference}.md"] = await get("desktop", reference)
        description, body = split_skill(core)
        return description, body, files


def split_skill(text: str) -> tuple[str, str]:
    """A SKILL.md's description from its front matter, and its body."""
    _, frontmatter, body = text.split("---\n", 2)
    lines, description = frontmatter.splitlines(), []
    for i, line in enumerate(lines):
        if line.startswith("description:"):
            rest = line.partition(":")[2].strip()
            if rest in (">", "|", ""):
                folded = []
                for x in lines[i + 1 :]:
                    if not x.startswith(" "):
                        break
                    folded.append(x.strip())
                description = folded
            else:
                description = [rest]
            break
    return " ".join(description), body.lstrip("\n")


def _timeout(arguments: dict[str, Any]) -> float:
    for key in ("timeout_ms", "timeout"):
        if isinstance(arguments.get(key), int):
            return arguments[key] / 1000 + 30
    return DEFAULT_TIMEOUT_S


def _env() -> dict[str, str]:
    return {k: v for k, v in os.environ.items() if k in ("HOME", "USER", "PATH", "TMPDIR", "LANG")}


if __name__ == "__main__":
    if sys.argv[1:] == ["--write-manifest"]:
        MANIFEST.write_text(json.dumps(manifest(), indent=1) + "\n")
        print(f"wrote {MANIFEST}: {len(TOOLS)} tools")
