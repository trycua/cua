"""Vercel's agent-desktop as an AgentEnv computer driver on macOS.

agent-desktop (https://github.com/vercel-labs/agent-desktop, Apache-2.0) is a CLI with no MCP server: it reads an app's
accessibility tree, prints it with a ref for each element, and acts on a ref. This driver serves its whole command
surface as tools, derived from the CLI's own command tree rather than listed by hand: every leaf command is one tool,
named ``desktop_<command>`` (``desktop_click``, ``desktop_double_click``, ``desktop_skills_get``), and its input schema
comes from the command's arguments and flags. A call runs the pinned binary as a child process in the VM's GUI session,
as its user, and returns the command's JSON envelope verbatim. Agents get:

* the derived tools (``tools.json`` pins them for this version);
* agent-desktop's own skill, as the pinned binary embeds it (``agent-desktop skills get desktop``), as the instructions;
* the neutral ``computer_*`` tools of ``macos_computer.py``, which an env hides with its ``[tools]`` list.

How the catalog is derived is in ``desktop_catalog.py``: it walks ``--help`` recursively. The driver derives it from
the installed binary when it starts and caches it under the binary's sha256, so a binary is introspected once.

Pinned: agent-desktop 1.0.2, ``aarch64-apple-darwin``. The ready checks fail clearly when the executable isn't the
pinned one, when the tool list derived from it differs from ``tools.json``, or when macOS hasn't granted Accessibility
and Screen Recording to it. That keeps a row reproducible: a different binary can't quietly change the tools.

To move to another version: point ``AGENT_DESKTOP_BIN`` at the new binary and run ``python driver.py --write-manifest``.
It rewrites ``tools.json`` and prints the constants to change here; read the diff of ``tools.json``.

Driver-level global flags (``AGENT_DESKTOP_GLOBAL_FLAGS``, split like a shell line, for example ``--headed``) are added
to every tool call. Each call also has a time limit: the call's own timeout argument plus a margin, else 60 s.

Needs ``agentenv-framework-protocol[computer]`` where it runs, and the executable at ``EXECUTABLE``.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shlex
import signal
import sys
import tempfile
from pathlib import Path
from typing import Any

from agentenv_protocol import EnvironmentTool, ForwardedTool, server_instructions
from agentenv_protocol.computer import ReadyCheck, computer_driver
from mcp.types import CallToolResult, TextContent

sys.path.insert(0, str(Path(__file__).parent))
import desktop_catalog as catalog  # noqa: E402
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
#: Where a binary's derived catalog is cached, one file for each sha256.
CACHE_DIR = Path(
    os.environ.get(
        "AGENT_DESKTOP_CATALOG_CACHE", str(Path(tempfile.gettempdir()) / "agentenv-agent-desktop")
    )
)
#: Flags added to every tool call, from the environment (for example ``--headed``).
GLOBAL_FLAGS: tuple[str, ...] = tuple(shlex.split(os.environ.get("AGENT_DESKTOP_GLOBAL_FLAGS", "")))
#: The skill texts the binary embeds, and the references of the core one.
SKILLS = ("desktop", "platform")
REFERENCES = ("commands-observation", "commands-interaction", "commands-system", "workflows")
DEFAULT_TIMEOUT_S = 60.0
MAX_TIMEOUT_S = 900.0
#: Margin on top of a command's own timeout argument, so its timeout answers before ours does.
TIMEOUT_MARGIN_S = 30.0
#: Arguments that bound how long a command may run, in milliseconds.
TIMEOUT_ARGUMENTS = ("timeout_ms", "timeout", "wait_timeout", "ms")
PERMISSIONS = ("accessibility", "screen_recording")


def pinned_document() -> dict:
    """``tools.json``: the version, the executable's sha256 and the pinned tools. A tool's input schema is built from
    its params, so the file holds each only once."""
    document = json.loads(MANIFEST.read_text())
    for tool in document["tools"]:
        tool["inputSchema"] = catalog.schema_of(tool["params"])
    return document


def with_globals(argv: list[str], flags: tuple[str, ...] | None = None) -> list[str]:
    """``argv`` with the driver-level flags added before any ``--``, except one the call already carries."""
    flags = GLOBAL_FLAGS if flags is None else flags
    present = {a.partition("=")[0] for a in argv[: argv.index("--") if "--" in argv else len(argv)]}
    extra: list[str] = []
    for flag in flags:
        if flag.partition("=")[0] not in present or not flag.startswith("-"):
            extra.append(flag)
    cut = argv.index("--") if "--" in argv else len(argv)
    return [*argv[:cut], *extra, *argv[cut:]]


def call_timeout(tool: dict, arguments: dict[str, Any]) -> float:
    """How long one call may run: its own timeout argument plus a margin, or the default; a command whose
    own default is longer (a batch) keeps that."""
    given = [
        arguments[k]
        for k in TIMEOUT_ARGUMENTS
        if isinstance(arguments.get(k), int) and not isinstance(arguments.get(k), bool)
    ]
    if given:
        return min(max(given) / 1000 + TIMEOUT_MARGIN_S, MAX_TIMEOUT_S)
    defaults = [
        p["schema"]["default"]
        for p in tool["params"]
        if p["name"] in ("timeout_ms", "timeout") and isinstance(p["schema"].get("default"), int)
    ]
    longest = max(defaults, default=0) / 1000 + TIMEOUT_MARGIN_S
    return min(max(DEFAULT_TIMEOUT_S, longest), MAX_TIMEOUT_S)


def error_envelope(command: str, code: str, message: str) -> str:
    """An envelope shaped like the CLI's own, for a failure the CLI could not report itself (it never ran, or was
    stopped)."""
    return json.dumps(
        {"ok": False, "command": command, "error": {"code": code, "message": message}}
    )


def envelope_result(
    code: int, out: str, err: str, *, command: str = "agent-desktop"
) -> CallToolResult:
    """The tool result of one command: its stdout verbatim, which is its JSON envelope. It is an error when the
    envelope says ``ok: false`` or the command exited non-zero, and then the exit code and stderr follow as a second
    block, so nothing the CLI said is lost."""
    try:
        failed = not json.loads(out).get("ok", code == 0)
    except (ValueError, AttributeError):
        failed = code != 0
    failed = failed or code != 0
    content = [
        TextContent(
            type="text",
            text=out.strip()
            or error_envelope(command, "NO_OUTPUT", f"exited {code} with no output"),
        )
    ]
    if failed and (err.strip() or code):
        content.append(
            TextContent(type="text", text=f"exit code: {code}\nstderr: {err.strip() or '(empty)'}")
        )
    return CallToolResult(isError=failed, content=content)


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


def catalog_check(pinned: dict, derived: list[dict]) -> ReadyCheck:
    """Whether the tools derived from the running binary are the pinned ones."""
    problems = catalog.diff_catalogs(pinned["tools"], derived)
    if pinned.get("version") != VERSION or pinned.get("executable_sha256") != EXECUTABLE_SHA256:
        problems.insert(0, f"tools.json pins {pinned.get('version')}, not {VERSION}")
    return ReadyCheck(
        "agent-desktop tools",
        not problems,
        f"{len(derived)} tools derived from the CLI, the pinned list"
        if not problems
        else "; ".join(problems)
        + " (python driver.py --write-manifest regenerates tools.json for a new version)",
    )


@computer_driver(name="agent-desktop", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
class AgentDesktopMacOS(MacComputerDriver):
    def __init__(self) -> None:
        super().__init__()
        self._pinned: list[ReadyCheck] | None = None
        self._skill: tuple[str, str, dict[str, str]] | None = None

    def forwarded_tools(self) -> list[ForwardedTool]:
        """One forwarded tool for each pinned tool: the binary's derived tools are checked against this list by
        the ready checks, so serving the pin is serving what the binary derives."""
        return [
            ForwardedTool(
                EnvironmentTool(
                    name=t["name"], description=t["description"], inputSchema=t["inputSchema"]
                ),
                self._forward(t),
            )
            for t in pinned_document()["tools"]
        ]

    def _forward(self, tool: dict):
        async def call(**arguments: Any) -> CallToolResult:
            try:
                argv = with_globals(catalog.build_argv(tool, arguments))
            except ValueError as e:
                return CallToolResult(
                    isError=True,
                    content=[
                        TextContent(
                            type="text",
                            text=error_envelope(" ".join(tool["command"]), "INVALID_ARGS", str(e)),
                        )
                    ],
                )
            code, out, err = await self.run_cli(argv, timeout_s=call_timeout(tool, arguments))
            return envelope_result(code, out, err, command=" ".join(tool["command"]))

        return call

    async def run_cli(self, argv: list[str], *, timeout_s: float) -> tuple[int, str, str]:
        """Run agent-desktop in the session, as its user, with its output. A command that can't start or runs out of
        time answers with an error envelope on stdout (exit 127 and 124), so a tool result is always JSON."""
        command = argv[0] if argv else "agent-desktop"
        try:
            proc = await asyncio.create_subprocess_exec(
                str(EXECUTABLE),
                *argv,
                stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=_env(),
                start_new_session=True,  # its helpers are killed with it
            )
        except OSError as e:
            message = f"agent-desktop can't start: {e}"
            return 127, error_envelope(command, "SPAWN_FAILED", message), message
        try:
            out, err = await asyncio.wait_for(proc.communicate(), timeout_s)
        except TimeoutError:
            _kill_group(proc)
            await proc.wait()
            message = f"agent-desktop {command} didn't finish within {timeout_s:.0f} s"
            return 124, error_envelope(command, "TOOL_TIMEOUT", message), message
        return proc.returncode or 0, out.decode(errors="replace"), err.decode(errors="replace")

    @server_instructions
    def agent_desktop_instructions(self) -> str:
        return (
            f"agent-desktop {VERSION}: each tool here runs one agent-desktop command, named `desktop_` and the command "
            "(`desktop_click` runs `agent-desktop click`, `desktop_skills_get` runs `agent-desktop skills get`). Its "
            "arguments are that command's arguments and flags, with dashes written as underscores. Where the guide that "
            "follows shows a shell command, call the tool instead. Every call returns the command's JSON envelope as the "
            "CLI printed it; a failure adds its exit code and stderr."
        )

    async def ready_checks(self) -> list[ReadyCheck]:
        """The pinned executable and its derived tools, checked until they pass once, and the TCC grants, checked
        every time: a reset can lose them, and then every action fails silently."""
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
            return [
                ReadyCheck("agent-desktop", False, f"{EXECUTABLE} isn't installed"),
                ReadyCheck("agent-desktop tools", False, "no binary to derive them from"),
            ]
        digest = await asyncio.to_thread(catalog.sha256_file, EXECUTABLE)
        code, out, _ = await self.run_cli(["version"], timeout_s=30)
        try:
            reported = json.loads(out)["data"]["version"]
        except (ValueError, KeyError, TypeError):
            reported = None
        pinned = digest == EXECUTABLE_SHA256 and reported == VERSION
        first = ReadyCheck(
            "agent-desktop",
            pinned,
            f"agent-desktop {VERSION}, the pinned executable"
            if pinned
            else f"sha256 {digest} and version {reported}, not the pinned {EXECUTABLE_SHA256} and {VERSION}",
        )
        if not pinned:
            return [
                first,
                ReadyCheck("agent-desktop tools", False, "not derived from an unpinned binary"),
            ]
        try:
            derived = await self.derive_catalog(digest)
            second = catalog_check(pinned_document(), derived)
        except (RuntimeError, OSError, ValueError) as e:
            second = ReadyCheck("agent-desktop tools", False, f"couldn't derive the tools: {e}")
        return [first, second]

    async def derive_catalog(self, digest: str) -> list[dict]:
        """The binary's tools: from the cache for this sha256, else derived from its ``--help`` tree and cached."""
        if (cached := catalog.load_cache(CACHE_DIR, digest)) is not None:
            return cached

        async def help_for(words: list[str]) -> str:
            code, out, err = await self.run_cli([*words, "--help"], timeout_s=30)
            if code or not out.strip():
                raise RuntimeError(
                    f"agent-desktop {' '.join([*words, '--help'])}: {err.strip() or out.strip() or code}"
                )
            return out

        derived = await catalog.derive(help_for)
        catalog.save_cache(CACHE_DIR, digest, derived)
        return derived

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


def _kill_group(proc: asyncio.subprocess.Process) -> None:
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        proc.kill()


def _env() -> dict[str, str]:
    return {
        k: v
        for k, v in os.environ.items()
        if k in ("HOME", "USER", "PATH", "TMPDIR", "LANG") or k.startswith("AGENT_DESKTOP_")
    }


async def write_manifest() -> None:
    """Derive the tools from the binary at ``EXECUTABLE`` and write ``tools.json`` for it."""
    driver = AgentDesktopMacOS()
    digest = await asyncio.to_thread(catalog.sha256_file, EXECUTABLE)
    code, out, err = await driver.run_cli(["version"], timeout_s=30)
    if code:
        raise SystemExit(f"{EXECUTABLE} didn't report its version: {err.strip() or out.strip()}")
    version = json.loads(out)["data"]["version"]
    CACHE_DIR.joinpath(f"catalog-{digest}.json").unlink(missing_ok=True)
    tools = await driver.derive_catalog(digest)
    document = catalog.pinned_document(
        version, digest, [{k: v for k, v in t.items() if k != "inputSchema"} for t in tools]
    )
    MANIFEST.write_text(json.dumps(document, indent=1) + "\n")
    print(f"wrote {MANIFEST}: {len(tools)} tools for agent-desktop {version}")
    if (version, digest) != (VERSION, EXECUTABLE_SHA256):
        print(
            f"set VERSION = {version!r} and EXECUTABLE_SHA256 = {digest!r} in driver.py "
            "(and ARCHIVE_SHA256, HELPER_SHA256, requirements and the image's provisioning for the new release)"
        )


if __name__ == "__main__":
    if sys.argv[1:] == ["--write-manifest"]:
        asyncio.run(write_manifest())
