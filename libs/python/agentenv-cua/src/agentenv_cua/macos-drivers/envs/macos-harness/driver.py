"""browser-use's macOS harness as an AgentEnv computer driver on macOS.

macos-harness (https://github.com/browser-use/macos-harness, MIT) is eleven shell commands that read an app as numbered
text and act on a number or a pixel, in the background. Its docs wrap it as a CLI, so this driver wraps each command as
a tool, as the harness's own ``--help`` and skill describe them, and runs it in the VM's GUI session as its user.
Agents get:

* its eleven tools (``tools.json`` pins them, written by ``pin_manifest.py`` from the harness's own tool table);
* the harness's own skill, as the installed package embeds it, as the instructions;
* a read's screenshot as an image beside its text: the harness writes it to a file in the VM, which the agent can't open;
* the neutral ``computer_*`` tools of ``macos_computer.py``, which an env hides with its ``[tools]`` list.

Pinned: macos-harness 0.2.1 (PyPI wheel sha256 ``f8e3251c...``; ``requirements.txt``). Its ready checks fail clearly
when the version or the skill text differs from the pin, when the harness's tool table drifts from ``tools.json``, or
when macOS hasn't granted Accessibility and Screen Recording to the app that starts it.

Telemetry is off (``DO_NOT_TRACK``). Needs ``agentenv-framework-protocol[computer]`` and ``macos-harness`` in the venv.
"""

from __future__ import annotations

import asyncio
import base64
import functools
import hashlib
import json
import os
import re
import sys
from pathlib import Path
from typing import Any

from agentenv_protocol import EnvironmentTool, ForwardedTool, server_instructions
from agentenv_protocol.computer import ReadyCheck, computer_driver
from mcp.types import CallToolResult, ImageContent, TextContent

sys.path.insert(0, str(Path(__file__).parent))
from macos_computer import MacComputerDriver  # noqa: E402

VERSION = "0.2.1"
WHEEL_SHA256 = "f8e3251c3c00ec20263e91df7fcc61c2a14d7defe28adaf0259f3376d373c127"
SKILL_SHA256 = "e9081fc7fe10daa5c750b2f2c504cf6e335395c06b508d1dbf2ca7ffc74e1052"
#: The harness's console script, in this driver's venv.
EXECUTABLE = Path(
    os.environ.get(
        "MACOS_HARNESS_BIN", str(Path(__file__).parent / "venv" / "bin" / "macos-harness")
    )
)
MANIFEST = Path(__file__).with_name("tools.json")
#: A read says where it kept its screenshot: ``Screenshot (1280x800 px): /path/App Screenshot ... .jpeg``.
SCREENSHOT = re.compile(r"^Screenshot \((\d+)x(\d+) px\): (.+)$", re.MULTILINE)
MIME = {".png": "image/png", ".jpeg": "image/jpeg", ".jpg": "image/jpeg"}
#: Flags that take their text from stdin when given ``-``.
CALL_TIMEOUT_S = 120.0
PERMISSIONS = ("accessibility", "screen_recording")


@functools.cache
def pinned() -> dict:
    return json.loads(MANIFEST.read_text())


def argv_for(spec: dict, arguments: dict[str, Any]) -> tuple[list[str], str | None]:
    """The command line of one call, and the text to send on stdin when a text flag holds ``-``, the harness's way
    to read it from there. Flags are written ``--name=value`` so a value that starts with a dash is still a value."""
    schema = spec["inputSchema"]
    unknown = sorted(set(arguments) - set(schema["properties"]))
    if unknown:
        raise ValueError(f"{spec['name']} takes no argument named {', '.join(unknown)}")
    if missing := [n for n in schema.get("required", []) if n not in arguments]:
        raise ValueError(f"{spec['name']} needs {', '.join(missing)}")
    argv, stdin = [spec["name"]], None
    for name, prop in schema["properties"].items():
        if name not in arguments or arguments[name] is None:
            continue
        value = arguments[name]
        if prop["type"] == "boolean":
            if value:
                argv.append(f"--{name}")
        elif name == spec.get("stdin") and value == "-":
            argv.append(f"--{name}")
            stdin = "-"
        else:
            argv.append(f"--{name}={value}")
    return argv, stdin


def doctor_check(report: dict) -> ReadyCheck:
    """Ready when ``macos-harness doctor`` reports Accessibility and Screen Recording granted. Its two optional grants
    (sending events to one app, Input Monitoring) don't count."""
    granted = report.get("granted") or {}
    missing = [n for n in PERMISSIONS if granted.get(n) is not True]
    who = report.get("app") or "the app that starts it"
    return ReadyCheck(
        "macOS permissions",
        not missing,
        "Accessibility and Screen Recording granted"
        if not missing
        else f"not granted to {who}: {', '.join(missing)} (the image's TCC rows don't match this install)",
    )


def tools_check(help_text: str, spec_tools: list[dict]) -> ReadyCheck:
    """Whether the harness's ``--help`` still teaches exactly the pinned tools."""
    listed = set(re.findall(r"^  macos-harness ([a-z_]+)", help_text, flags=re.MULTILINE))
    expected = {t["name"] for t in spec_tools}
    differ = sorted(listed ^ expected)
    return ReadyCheck(
        "macos-harness tools",
        not differ,
        f"{len(expected)} tools, as pinned"
        if not differ
        else f"these tools differ from tools.json: {', '.join(differ)}",
    )


def attach_screenshot(text: str, read: Any) -> list[Any]:
    """The tool result's content: the text, and the screenshot a read kept, as an image, when ``read`` finds it."""
    content: list[Any] = [TextContent(type="text", text=text)]
    if match := SCREENSHOT.search(text):
        path = Path(match.group(3).strip())
        if mime := MIME.get(path.suffix.lower()):
            try:
                content.append(
                    ImageContent(
                        type="image", data=base64.b64encode(read(path)).decode(), mimeType=mime
                    )
                )
            except OSError:
                pass
    return content


@computer_driver(name="macos-harness", supports=[{"os": {"type": "macos"}, "arch": ["arm64"]}])
class MacosHarness(MacComputerDriver):
    def __init__(self) -> None:
        super().__init__()
        self._pinned: list[ReadyCheck] | None = None
        self._skill: tuple[str, str, dict[str, str]] | None = None

    def forwarded_tools(self) -> list[ForwardedTool]:
        return [
            ForwardedTool(
                EnvironmentTool(
                    name=t["name"], description=t["description"], inputSchema=t["inputSchema"]
                ),
                functools.partial(self._forward, t),
            )
            for t in pinned()["tools"]
        ]

    async def _forward(self, spec: dict, /, **arguments: Any) -> CallToolResult:
        try:
            argv, stdin = argv_for(spec, arguments)
        except ValueError as e:
            return CallToolResult(isError=True, content=[TextContent(type="text", text=str(e))])
        code, out, err = await self.run_cli(argv, stdin=stdin)
        text = (out + err).strip() or f"macos-harness exited {code}"
        content = (
            attach_screenshot(text, Path.read_bytes)
            if not code
            else [TextContent(type="text", text=text)]
        )
        return CallToolResult(isError=bool(code), content=content)

    async def run_cli(
        self, argv: list[str], *, stdin: str | None = None, timeout_s: float = CALL_TIMEOUT_S
    ) -> tuple[int, str, str]:
        """Run the harness in the session, as its user."""
        try:
            proc = await asyncio.create_subprocess_exec(
                str(EXECUTABLE),
                *argv,
                stdin=asyncio.subprocess.PIPE if stdin is not None else asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=_env(),
            )
        except OSError as e:
            return 127, "", f"macos-harness can't start: {e}"
        try:
            out, err = await asyncio.wait_for(
                proc.communicate(stdin.encode() if stdin is not None else None), timeout_s
            )
        except TimeoutError:
            proc.kill()
            await proc.wait()
            return 124, "", f"macos-harness {argv[0]} didn't finish within {timeout_s:.0f} s"
        return proc.returncode or 0, out.decode(errors="replace"), err.decode(errors="replace")

    @server_instructions
    def harness_instructions(self) -> str:
        return (
            f"macos-harness {VERSION}: each tool here is the harness command of the same name, and its arguments are "
            "that command's flags. Where the skill that follows shows a shell command, call the tool instead. A read's "
            "screenshot comes back as an image beside its text."
        )

    async def ready_checks(self) -> list[ReadyCheck]:
        """The pinned version, skill and tools, checked until they pass once, and the TCC grants, checked every time:
        a reset can lose them, and then every action fails silently."""
        if self._pinned is None or not all(c.ok for c in self._pinned):
            self._pinned = await self._pinned_checks()
        _, out, _ = await self.run_cli(["doctor"], timeout_s=60)
        try:
            report = json.loads(out)
        except ValueError:
            report = {}
        return [*self._pinned, doctor_check(report)]

    async def _pinned_checks(self) -> list[ReadyCheck]:
        if not EXECUTABLE.is_file():
            return [ReadyCheck("macos-harness", False, f"{EXECUTABLE} isn't installed")]
        _, version, _ = await self.run_cli(["--version"], timeout_s=30)
        _, skill, _ = await self.run_cli(["skill"], timeout_s=30)
        digest = hashlib.sha256(skill.encode()).hexdigest()
        ok = version.strip() == VERSION and digest == SKILL_SHA256
        _, help_text, _ = await self.run_cli(["--help"], timeout_s=30)
        return [
            ReadyCheck(
                "macos-harness",
                ok,
                f"macos-harness {VERSION}, the pinned version and skill"
                if ok
                else f"version {version.strip()!r} and skill sha256 {digest}, not the pinned {VERSION} and {SKILL_SHA256}",
            ),
            tools_check(help_text, pinned()["tools"]),
        ]

    async def bundled_skill(self) -> tuple[str, str, dict[str, str]] | None:
        """The harness's own skill, as the package embeds it."""
        if self._skill is None:
            code, text, err = await self.run_cli(["skill"], timeout_s=30)
            if code:
                raise RuntimeError(f"macos-harness skill: {err.strip()}")
            _, frontmatter, body = text.split("---\n", 2)
            description = next(
                (
                    line.partition(":")[2].strip()
                    for line in frontmatter.splitlines()
                    if line.startswith("description:")
                ),
                "",
            )
            self._skill = (description, body.lstrip("\n"), {})
        return self._skill


def _env() -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k in ("HOME", "USER", "PATH", "TMPDIR", "LANG")}
    return {**env, "DO_NOT_TRACK": "1"}
