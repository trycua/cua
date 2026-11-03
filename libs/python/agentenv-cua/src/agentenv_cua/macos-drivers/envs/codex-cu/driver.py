"""OpenAI's Codex Computer Use ("Sky") as an AgentEnv computer driver on macOS.

Codex Computer Use ships inside the ChatGPT app as the ``unified-computer-use`` plugin, whose MCP server, ``cua_repl``,
serves ``js`` (run JavaScript against the ``cua`` API: ``cua.getApp(...)``, snapshots, clicks) and ``js_reset``. This
driver runs that server unchanged, as the plugin's own ``.mcp.json`` starts it, except that only the ``computer``
surface is on (no browser), and forwards its two model-facing tools. The REPL keeps state between calls, so each agent
session gets one server process that lives as long as the driver does.

Agents get: those two tools (``mcp.json`` pins them as the pinned app's server lists them, and its instructions), and the
neutral ``computer_*`` tools of ``macos_computer.py``, which an env hides with its ``[tools]`` list. There is no skill:
the arm runs as OpenAI ships it.

Precondition, not something this driver can do: **a ChatGPT login inside the VM**. Sky needs the Codex CLI of the pinned
ChatGPT app logged in with a ChatGPT account (``codex login`` as the VM's session user, once, when the image is built;
the login is a file in that user's ``~/.codex``). Neither the app nor any credential is in this repository or in a public
image, and Scale needs an account of its own. The ready check "ChatGPT login" fails with that instruction until it holds.

Pinned: ChatGPT/Codex app 26.930.51102 (zip sha256 below), its plugin of the same version, and ``mcp.json``.
"""

from __future__ import annotations

import asyncio
import contextlib
import hashlib
import json
import os
import plistlib
import re
import sys
from pathlib import Path
from typing import Any

from agentenv_protocol import EnvironmentTool, ForwardedTool, ToolContext, server_instructions
from agentenv_protocol.computer import DeviceUnavailable, ReadyCheck, computer_driver
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.types import CallToolResult, TextContent

sys.path.insert(0, str(Path(__file__).parent))
from macos_computer import MacComputerDriver  # noqa: E402

APP_VERSION = "26.930.51102"
APP = Path(os.environ.get("CODEX_CU_APP", "/Applications/ChatGPT.app"))
APP_ZIP_URL = (
    f"https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-{APP_VERSION}.zip"
)
APP_ZIP_SHA256 = "7001df293fd478b2a4e2d8b5f4ea3aae43a2e21d168960256b2f31ac4ee43f6f"
#: sha256 of ``ChatGPT.app/Contents/MacOS/ChatGPT`` in that zip.
APP_EXECUTABLE_SHA256 = "1f2989ac3d702e48c740fe4c54b1b6279f9e48e0e4c0ead7d4501464e2ea976e"
CODEX_CLI = APP / "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
#: Where the app puts the plugin once it has run, and the MCP server entry inside it.
PLUGIN_ROOT = Path.home() / ".codex/plugins/cache/openai-bundled/unified-computer-use"
SERVER = "cua_repl"
SURFACES = "computer"
MANIFEST = Path(__file__).with_name("mcp.json")
LOGIN_INSTRUCTION = (
    "Codex Computer Use needs a ChatGPT login in this VM. Log in once as the VM's session user, with the pinned app's "
    f"CLI: `{CODEX_CLI} login` (a ChatGPT account, not an API key), then check `login status`."
)
CALL_TIMEOUT_S = 600.0
START_TIMEOUT_S = 120.0


def version_key(path: Path) -> tuple[int, ...]:
    return tuple(int(part) for part in re.findall(r"\d+", path.name))


def newest_plugin_dir(root: Path = PLUGIN_ROOT) -> Path | None:
    """The newest version folder of the plugin that has a ``.mcp.json``."""
    if not root.is_dir():
        return None
    found = [p for p in root.iterdir() if (p / ".mcp.json").is_file()]
    return max(found, key=version_key) if found else None


def server_parameters(plugin_dir: Path, base_env: dict[str, str]) -> StdioServerParameters:
    """The plugin's ``cua_repl`` entry as shipped (same command, arguments and environment), with the computer surface
    only, so no browser can be touched."""
    entry = json.loads((plugin_dir / ".mcp.json").read_text())["mcpServers"][SERVER]
    env = {**base_env, **entry.get("env", {}), "CUA_REPL_ENABLED_SURFACES": SURFACES}
    return StdioServerParameters(
        command=entry["command"], args=list(entry.get("args", [])), env=env
    )


def app_check(version: str | None, executable_sha256: str | None) -> ReadyCheck:
    ok = version == APP_VERSION and executable_sha256 == APP_EXECUTABLE_SHA256
    return ReadyCheck(
        "ChatGPT app",
        ok,
        f"ChatGPT {APP_VERSION}, the pinned build"
        if ok
        else f"version {version} and executable sha256 {executable_sha256}, not the pinned {APP_VERSION} and {APP_EXECUTABLE_SHA256}",
    )


def plugin_check(plugin_dir: Path | None) -> ReadyCheck:
    if plugin_dir is None:
        return ReadyCheck(
            "Computer Use plugin",
            False,
            f"no unified-computer-use plugin under {PLUGIN_ROOT}: the ChatGPT app installs it when it first runs; "
            + LOGIN_INSTRUCTION,
        )
    ok = plugin_dir.name == APP_VERSION
    return ReadyCheck(
        "Computer Use plugin",
        ok,
        f"unified-computer-use {plugin_dir.name}"
        if ok
        else f"plugin {plugin_dir.name}, not the pinned {APP_VERSION}",
    )


def login_check(code: int, output: str) -> ReadyCheck:
    """The login precondition: ``codex login status`` exits 0 and says it's a ChatGPT login."""
    ok = code == 0 and "chatgpt" in output.lower()
    return ReadyCheck(
        "ChatGPT login",
        ok,
        "logged in with ChatGPT"
        if ok
        else f"not logged in with a ChatGPT account ({output.strip() or f'exit {code}'}). {LOGIN_INSTRUCTION}",
    )


def tools_check(listed: list[dict], pinned: list[dict]) -> ReadyCheck:
    """Whether the running server lists the pinned tools, with their pinned input schemas and descriptions."""
    running = {t["name"]: (t.get("description"), t.get("inputSchema")) for t in listed}
    expected = {t["name"]: (t["description"], t["inputSchema"]) for t in pinned}
    differ = sorted(n for n in expected if running.get(n) != expected[n])
    return ReadyCheck(
        "cua_repl tools",
        not differ,
        f"{len(expected)} tools, as pinned"
        if not differ
        else f"these tools differ from mcp.json: {', '.join(differ)}",
    )


class Repl:
    """One ``cua_repl`` server process, owned by a task of its own: MCP's stdio client must be entered and left in
    one task, and a call can come from any. Requests go through a queue; the process starts on the first."""

    def __init__(self, connect) -> None:
        self._connect = connect
        self._queue: asyncio.Queue = asyncio.Queue()
        self._task: asyncio.Task | None = None
        self._ready = asyncio.Event()
        self._failure: BaseException | None = None

    async def call(self, method: str, *args: Any) -> Any:
        if self._task is None:
            self._task = asyncio.create_task(self._run())
        await asyncio.wait_for(self._ready.wait(), START_TIMEOUT_S)
        if self._failure is not None:
            raise DeviceUnavailable(f"the cua_repl server didn't start: {self._failure}")
        future: asyncio.Future = asyncio.get_running_loop().create_future()
        await self._queue.put((method, args, future))
        return await asyncio.wait_for(future, CALL_TIMEOUT_S)

    async def close(self) -> None:
        if self._task is not None:
            await self._queue.put(None)
            with contextlib.suppress(Exception):
                await asyncio.wait_for(self._task, 15)

    async def _run(self) -> None:
        try:
            async with self._connect() as session:
                self._ready.set()
                while (item := await self._queue.get()) is not None:
                    method, args, future = item
                    try:
                        future.set_result(await getattr(session, method)(*args))
                    except Exception as e:  # the caller gets it; the server stays up
                        future.set_exception(e)
        except Exception as e:
            self._failure = e
        finally:
            self._ready.set()
            while not self._queue.empty():  # a call that arrived as the server died
                item = self._queue.get_nowait()
                if item is not None and not item[2].done():
                    item[2].set_exception(DeviceUnavailable("the cua_repl server stopped"))


@contextlib.asynccontextmanager
async def stdio_session(params: StdioServerParameters):
    async with stdio_client(params) as (read, write), ClientSession(read, write) as session:
        await session.initialize()
        yield session


@computer_driver(name="codex-cu", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
class CodexComputerUse(MacComputerDriver):
    def __init__(self) -> None:
        super().__init__()
        self._repls: dict[str | None, Repl] = {}
        self._pinned: list[ReadyCheck] | None = None
        self._live: ReadyCheck | None = None

    def forwarded_tools(self) -> list[ForwardedTool]:
        return [
            ForwardedTool(
                EnvironmentTool(
                    name=t["name"], description=t["description"], inputSchema=t["inputSchema"]
                ),
                self._forward(t["name"]),
                t.get("annotations") or None,
            )
            for t in _manifest()["tools"]
        ]

    def _forward(self, name: str):
        async def call(**arguments: Any) -> CallToolResult:
            try:
                return await self._repl().call("call_tool", name, arguments)
            except DeviceUnavailable as e:
                return _refused(e.message)
            except TimeoutError:
                return _refused(f"{name} didn't finish within {CALL_TIMEOUT_S:.0f} s")
            except Exception as e:
                return _refused(f"{name}: {type(e).__name__}: {e}")

        return call

    def _repl(self) -> Repl:
        """The caller's own REPL server: its JavaScript bindings are its own."""
        caller = ToolContext.current().caller.session
        if caller not in self._repls:
            plugin = newest_plugin_dir()
            if plugin is None:
                raise DeviceUnavailable(plugin_check(None).detail)
            params = server_parameters(plugin, _base_env())
            self._repls[caller] = Repl(lambda: stdio_session(params))
        return self._repls[caller]

    @server_instructions
    def cua_repl_instructions(self) -> str:
        return _manifest()["instructions"]

    async def ready_checks(self) -> list[ReadyCheck]:
        """The pinned app, plugin and tools, checked until they pass once; the login, checked every time, since it's
        the one precondition that lives outside the image; and that Computer Use answers, once."""
        if self._pinned is None or not all(c.ok for c in self._pinned):
            self._pinned = await self._pinned_checks()
        code, output = await self._login_status()
        login = login_check(code, output)
        checks = [*self._pinned, login]
        if login.ok and all(c.ok for c in self._pinned):
            if self._live is None or not self._live.ok:
                self._live = await self._answers()
            checks.append(self._live)
        return checks

    async def _pinned_checks(self) -> list[ReadyCheck]:
        version, digest = await asyncio.to_thread(_app_identity)
        plugin = newest_plugin_dir()
        checks = [app_check(version, digest), plugin_check(plugin)]
        if plugin is not None:
            params = server_parameters(plugin, _base_env())
            try:
                async with asyncio.timeout(START_TIMEOUT_S):
                    async with stdio_session(params) as session:
                        listed = [t.model_dump() for t in (await session.list_tools()).tools]
                checks.append(tools_check(listed, _manifest()["tools"]))
            except Exception as e:
                checks.append(
                    ReadyCheck("cua_repl tools", False, f"the server didn't list its tools: {e}")
                )
        return checks

    async def _login_status(self) -> tuple[int, str]:
        try:
            proc = await asyncio.create_subprocess_exec(
                str(CODEX_CLI),
                "login",
                "status",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
                stdin=asyncio.subprocess.DEVNULL,
                env=_base_env(),
            )
            out, _ = await asyncio.wait_for(proc.communicate(), 30)
        except (OSError, TimeoutError) as e:
            return 127, str(e)
        return proc.returncode or 0, out.decode(errors="replace")

    async def _answers(self) -> ReadyCheck:
        """Computer Use answers: the first call of a REPL is one API call, here the app list's state."""
        try:
            result = await self._repl_for("ready-check").call(
                "call_tool", "js", {"code": "await cua.getState();"}
            )
        except Exception as e:
            return ReadyCheck("Computer Use answers", False, f"cua.getState() failed: {e}")
        ok = not result.isError
        text = " ".join(getattr(c, "text", "") for c in result.content)[:200]
        return ReadyCheck(
            "Computer Use answers",
            ok,
            "cua.getState() answered" if ok else f"cua.getState() failed: {text}",
        )

    def _repl_for(self, key: str) -> Repl:
        plugin = newest_plugin_dir()
        if key not in self._repls:
            params = server_parameters(plugin, _base_env())
            self._repls[key] = Repl(lambda: stdio_session(params))
        return self._repls[key]


def _manifest() -> dict:
    return json.loads(MANIFEST.read_text())


def _refused(message: str) -> CallToolResult:
    return CallToolResult(isError=True, content=[TextContent(type="text", text=message)])


def _base_env() -> dict[str, str]:
    return {
        k: v
        for k, v in os.environ.items()
        if k in ("HOME", "USER", "PATH", "TMPDIR", "LANG", "CODEX_HOME")
    }


def _app_identity() -> tuple[str | None, str | None]:
    """The app's version and the sha256 of its executable, None for what isn't there."""
    try:
        version = plistlib.loads((APP / "Contents/Info.plist").read_bytes()).get(
            "CFBundleShortVersionString"
        )
    except (OSError, plistlib.InvalidFileException):
        version = None
    try:
        digest = hashlib.sha256((APP / "Contents/MacOS/ChatGPT").read_bytes()).hexdigest()
    except OSError:
        digest = None
    return version, digest
