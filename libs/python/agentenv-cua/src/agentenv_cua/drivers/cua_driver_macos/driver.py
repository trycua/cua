"""cua-driver as an AgentEnv computer driver on macOS.

The macOS counterpart of AgentEnv's Linux ``cua-driver`` driver. cua-driver runs in the VM's GUI session as a daemon,
started from ``CuaDriver.app``, whose bundle (``com.trycua.driver``) holds the Accessibility and Screen Recording
grants. Agents get what cua-driver's own MCP server serves at the pinned version (``mcp.json``): its instructions, its
tools beside the ``computer_*`` tools and its skill resources. The ``computer/v1`` operations are cua-driver's
desktop-scope calls, made in a cua-driver session of the framework's own.

Run as a script, this file starts the daemon (the image's LaunchAgent does that at login). Served with
``python -m agentenv_protocol.computer.serve driver.py:CuaDriverMacOS``, it is the driver.

It needs ``agentenv-framework-protocol[computer]`` where it runs, and CuaDriver.app at ``APP``.
"""

from __future__ import annotations

import asyncio
import base64
import contextlib
import functools
import hashlib
import json
import os
import subprocess
import tempfile
import uuid
from pathlib import Path
from typing import Any

from agentenv_protocol import (
    EnvironmentTool,
    ForwardedTool,
    ToolContext,
    resource,
    server_instructions,
)
from agentenv_protocol.computer import (
    PRINTABLE_KEYS,
    ComputerDriverEnvironment,
    DeliveryFailed,
    DeviceUnavailable,
    Display,
    Image,
    ReadyCheck,
    Unsupported,
    capture,
    click,
    computer_driver,
    display,
    drag,
    press,
    run_in_session,
    scroll,
    type_text,
    write_files,
)
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.types import CallToolResult, TextContent
from pydantic import AnyUrl

#: The app whose bundle owns cua-driver's TCC grants, and its executable.
APP = Path(os.environ.get("CUA_DRIVER_APP", "/Applications/CuaDriver.app"))
EXECUTABLE = APP / "Contents" / "MacOS" / "cua-driver"
#: sha256 of ``CuaDriver.app/Contents/MacOS/cua-driver`` in ``cua-driver-rs-0.38.0-darwin-arm64.tar.gz``
#: (tarball sha256 5f71191c082ca07c9dfc13b7c9a6eec60a82818c245a93534cc0eb7911471d72).
VERSION = "0.38.0"
EXECUTABLE_SHA256 = "b4d974a9c1c92a700cd7be7ede987aea4a1e6835e37039ce3d628b71bd5f0ca8"
#: Where the daemon listens: in the session user's caches, which only that user reads.
SOCKET = Path.home() / "Library" / "Caches" / "agentenv" / "cua-driver.sock"
#: What cua-driver's MCP server (``cua-driver mcp``) says at connect and lists at the pinned version on macOS;
#: regenerate with ``pin_manifest.py`` on a macOS VM. Its tools differ from Linux's, so each OS has its own pin.
MANIFEST = Path(__file__).with_name("mcp.json")
#: No telemetry and no update checks.
QUIET = {
    "DO_NOT_TRACK": "1",
    "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
    "CUA_DRIVER_RS_UPDATE_CHECK": "false",
}
#: W3C modifier keys and the names cua-driver holds them by on macOS (Meta is Command).
MODIFIERS = {"Control": "ctrl", "Shift": "shift", "Alt": "option", "Meta": "cmd", "Super": "cmd"}
#: W3C key values and cua-driver's names for them; any printable ASCII character is pressed as itself.
NAMED = {
    "Enter": "return",
    "Tab": "tab",
    "Escape": "escape",
    " ": "space",
    "Backspace": "delete",
    "Delete": "forwarddelete",
    "Home": "home",
    "End": "end",
    "PageUp": "pageup",
    "PageDown": "pagedown",
    "ArrowUp": "up",
    "ArrowDown": "down",
    "ArrowLeft": "left",
    "ArrowRight": "right",
    **{f"F{n}": f"f{n}" for n in range(1, 13)},
}
#: What cua-driver's ``check_permissions`` must report true: both TCC grants, and a capture that actually works.
PERMISSIONS = ("accessibility", "screen_recording", "screen_recording_capturable")
SKILL_TIMEOUT_S = 60
REPLY_LIMIT = 64 << 20
#: How much of a command's output computer-exec keeps.
EXEC_OUTPUT_BYTES = 1 << 20


@computer_driver(name="cua-driver", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
class CuaDriverMacOS(ComputerDriverEnvironment):
    buttons = ("left", "right", "middle")
    max_click_count = 3
    max_scroll_amount = 50
    keys = (*MODIFIERS, *NAMED, *PRINTABLE_KEYS)

    def __init__(self) -> None:
        super().__init__()
        self._session = uuid.uuid4().hex
        self._callers: dict[str | None, str] = {}
        self._pinned: list[ReadyCheck] | None = None
        self._skill: tuple[str, str, dict[str, str]] | None = None
        self._texts: dict[str, str] | None = None

    def forwarded_tools(self) -> list[ForwardedTool]:
        return [
            ForwardedTool(
                EnvironmentTool(
                    name=t["name"], description=t["description"], inputSchema=t["inputSchema"]
                ),
                functools.partial(self._forward, t["name"]),
                t.get("annotations") or None,
                t.get("outputSchema"),
            )
            for t in _manifest()["tools"]
        ]

    @resource("skill://cua-driver/{file}", listed=lambda env: _manifest()["resources"])
    async def skill_file(self, file: str) -> str:
        """One of cua-driver's skill files as its own MCP server serves it."""
        if self._texts is None:
            self._texts = await _read_resources([r["uri"] for r in _manifest()["resources"]])
        return self._texts[f"skill://cua-driver/{file}"]

    @server_instructions
    def cua_driver_instructions(self) -> str:
        return _manifest()["instructions"]

    async def ready_checks(self) -> list[ReadyCheck]:
        """The pinned executable and tool list, checked until they pass once, and the TCC grants, checked every time:
        a reset can lose them, and then every action fails silently."""
        if self._pinned is None or not all(check.ok for check in self._pinned):
            self._pinned = await self._pinned_checks()
        return [*self._pinned, await self._permissions_check()]

    async def bundled_skill(self) -> tuple[str, str, dict[str, str]]:
        if self._skill is None:
            self._skill = await asyncio.to_thread(_skill_pack)
        return self._skill

    @display
    async def screen(self) -> Display:
        size = (await self._call("get_screen_size", DeviceUnavailable))["structuredContent"]
        return Display(size["width"], size["height"], size.get("scale_factor") or 1.0)

    @capture
    async def grab(self) -> Image:
        result = await self._call("get_desktop_state", DeviceUnavailable)
        shot, state = (
            next(c for c in result["content"] if c["type"] == "image"),
            result["structuredContent"],
        )
        return Image(
            state["screenshot_width"],
            state["screenshot_height"],
            base64.b64decode(shot["data"]),
            "png",
        )

    @click
    async def click_at(self, x, y, button, count, modifiers) -> None:
        await self._send(
            "click", x=x, y=y, button=button, count=count, modifier=_modifiers(modifiers)
        )

    @drag
    async def drag_along(self, path, button, duration_ms, modifiers) -> None:
        (x0, y0), (x1, y1) = path[0], path[-1]
        await self._send(
            "drag",
            from_x=x0,
            from_y=y0,
            to_x=x1,
            to_y=y1,
            button=button,
            duration_ms=duration_ms,
            modifier=_modifiers(modifiers),
        )

    @scroll
    async def scroll_at(self, x, y, direction, amount, modifiers) -> None:
        if modifiers:
            raise Unsupported("cua-driver scrolls with no keys held")
        await self._send("scroll", x=x, y=y, direction=direction, amount=amount)

    @type_text
    async def type_in(self, text) -> None:
        await self._send("type_text", text=text)

    @press
    async def press_keys(self, keys) -> None:
        *held, key = keys
        await self._send(
            "press_key", modifiers=_modifiers(held), key=MODIFIERS.get(key) or NAMED.get(key, key)
        )

    @run_in_session(os="macos")
    async def run_command(self, argv, cwd, env, timeout_s):
        """computer-exec on macOS: the driver runs in the GUI session as its user, so a command it starts does too."""
        from agentenv_protocol.computer import (
            _linux,  # POSIX: nothing in it is Linux-specific
        )

        return await _linux.run_in_session(
            argv,
            cwd,
            env,
            timeout_s,
            output_bytes=getattr(self.settings, "exec_output_bytes", EXEC_OUTPUT_BYTES),
        )

    @write_files(os="macos")
    async def write_home_files(self, files) -> None:
        from agentenv_protocol.computer import _linux

        await asyncio.to_thread(_linux.write_files, files)

    async def _forward(self, tool: str, /, **arguments: Any) -> CallToolResult:
        """A call to one of cua-driver's tools in its caller's own cua-driver session."""
        session = self._callers.setdefault(ToolContext.current().caller.session, uuid.uuid4().hex)
        try:
            reply = await _request(
                {"method": "call", "name": tool, "args": arguments, "session_id": session}
            )
        except DeviceUnavailable as e:
            return _refused(e.message)
        if not reply.get("ok"):
            return _refused(
                reply.get("error") or "daemon reported failure", reply.get("exit_code") or 1
            )
        return CallToolResult.model_validate(reply["result"])

    async def _send(self, name: str, **args: Any) -> None:
        await self._call(name, DeliveryFailed, scope="desktop", **args)

    async def _call(self, name: str, failure: type[Exception], **args: Any) -> dict:
        try:
            reply = await _request(
                {"method": "call", "name": name, "args": args, "session_id": self._session}
            )
        except _ReplyLost as e:
            raise failure(e.message) from None
        if not reply.get("ok"):
            raise failure(f"cua-driver {name}: {reply.get('error')}")
        result = reply["result"]
        if result.get("isError"):
            raise failure(f"cua-driver {name}: {_text(result)}")
        return result

    async def _pinned_checks(self) -> list[ReadyCheck]:
        try:
            listed = (await _request({"method": "list"}))["result"]["tools"]
        except DeviceUnavailable:
            return [ReadyCheck("cua-driver", False, "its daemon isn't answering yet")]
        digest = await asyncio.to_thread(_executable_sha256)
        return [
            ReadyCheck(
                "cua-driver",
                digest == EXECUTABLE_SHA256,
                (
                    f"cua-driver {VERSION}, the pinned executable"
                    if digest == EXECUTABLE_SHA256
                    else f"its executable's sha256 is {digest}, not the pinned {EXECUTABLE_SHA256}"
                ),
            ),
            tool_list_check(listed, _manifest()["tools"]),
        ]

    async def _permissions_check(self) -> ReadyCheck:
        try:
            result = await self._call("check_permissions", DeviceUnavailable)
        except DeviceUnavailable as e:
            return ReadyCheck("macOS permissions", False, str(e))
        return permissions_check(result.get("structuredContent") or {})


def tool_list_check(listed: list[dict], pinned: list[dict]) -> ReadyCheck:
    """Whether the running daemon lists exactly the pinned tools, with the pinned input schemas."""
    running = {t["name"]: t.get("input_schema", t.get("inputSchema")) for t in listed}
    expected = {t["name"]: t["inputSchema"] for t in pinned}
    differ = sorted(
        name for name in running.keys() | expected.keys() if running.get(name) != expected.get(name)
    )
    return ReadyCheck(
        "cua-driver tools",
        not differ,
        (
            f"{len(running)} tools, as pinned"
            if not differ
            else f"these tools differ from mcp.json: {', '.join(differ)}"
        ),
    )


def permissions_check(state: dict) -> ReadyCheck:
    """Ready when ``check_permissions`` reports every TCC grant cua-driver needs as granted."""
    missing = [name for name in PERMISSIONS if not _granted(state.get(name))]
    return ReadyCheck(
        "macOS permissions",
        not missing,
        (
            "Accessibility and Screen Recording granted, capture works"
            if not missing
            else f"not granted: {', '.join(missing)} (the image's TCC rows don't match this CuaDriver.app)"
        ),
    )


def _granted(value: Any) -> bool:
    return value is True


class _ReplyLost(DeviceUnavailable):
    """The connection dropped after a request was sent, so the daemon may have acted on it."""


async def _request(payload: dict) -> dict:
    try:
        reader, writer = await asyncio.open_unix_connection(str(SOCKET), limit=REPLY_LIMIT)
    except OSError as e:
        raise DeviceUnavailable(f"cua-driver's daemon isn't answering: {e}") from None
    try:
        writer.write((json.dumps(payload) + "\n").encode())
        await writer.drain()
        line = await reader.readline()
        if not line:
            raise ConnectionError("the daemon closed the connection")
        return json.loads(line)
    except (OSError, ValueError) as e:
        raise _ReplyLost(
            f"cua-driver's daemon stopped answering after the request was sent, which it may have acted "
            f"on: {e}"
        ) from None
    finally:
        writer.close()


async def _read_resources(uris: list[str]) -> dict[str, str]:
    server = StdioServerParameters(
        command=str(EXECUTABLE), args=["mcp", "--socket", str(SOCKET)], env={**os.environ, **QUIET}
    )
    async with stdio_client(server) as (read, write), ClientSession(read, write) as session:
        await session.initialize()
        return {uri: (await session.read_resource(AnyUrl(uri))).contents[0].text for uri in uris}


@functools.cache
def _manifest() -> dict:
    return json.loads(MANIFEST.read_text())


def _modifiers(keys) -> list[str]:
    if held := [key for key in keys if key not in MODIFIERS]:
        raise Unsupported(f"cua-driver holds only modifier keys, not {held}")
    return [MODIFIERS[key] for key in keys]


def _refused(message: str, exit_code: int = 1) -> CallToolResult:
    return CallToolResult(
        isError=True,
        content=[TextContent(type="text", text=message)],
        structuredContent={"code": "tool_invocation_failed", "exit_code": exit_code},
    )


def _text(result: dict) -> str:
    return " ".join(c.get("text", "") for c in result.get("content", []) if c.get("type") == "text")


def _executable_sha256() -> str:
    return hashlib.sha256(EXECUTABLE.read_bytes()).hexdigest()


def _skill_pack() -> tuple[str, str, dict[str, str]]:
    """cua-driver's skill pack as its executable embeds it, written offline by ``mcp-config`` into a home of its own."""
    with tempfile.TemporaryDirectory(prefix="cua-driver-skill-") as home:
        (Path(home) / ".claude").mkdir()
        subprocess.run(
            [str(EXECUTABLE), "mcp-config", "--client", "claude"],
            capture_output=True,
            check=True,
            timeout=SKILL_TIMEOUT_S,
            env={**os.environ, "HOME": home, **QUIET},
        )
        folder = Path(home) / ".cua-driver" / "skills" / "cua-driver"
        files = {path.name: path.read_text() for path in sorted(folder.iterdir()) if path.is_file()}
    _, frontmatter, body = files.pop("SKILL.md").split("---\n", 2)
    description = next(
        line.partition(":")[2].strip()
        for line in frontmatter.splitlines()
        if line.startswith("description:")
    )
    return description, body.lstrip("\n"), files


def _daemon() -> None:
    """Run cua-driver's daemon in the GUI session, from the user's home, on the driver's socket, with approvals off
    (nobody is at the computer to give them) and nothing sent to trycua. The process becomes CuaDriver.app's own
    executable, so macOS attributes it to the app's bundle and its TCC grants."""
    SOCKET.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with contextlib.suppress(FileNotFoundError):
        SOCKET.unlink()
    os.chdir(Path.home())
    executable = str(EXECUTABLE)
    os.execve(
        executable,
        [
            executable,
            "serve",
            "--socket",
            str(SOCKET),
            "--dangerously-bypass-approvals",
            "--no-overlay",
        ],
        {**os.environ, **QUIET},
    )


if __name__ == "__main__":
    _daemon()
