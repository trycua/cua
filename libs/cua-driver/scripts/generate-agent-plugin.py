#!/usr/bin/env python3
"""Generate the cross-vendor Cua Driver plugin package."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
DRIVER_ROOT = ROOT / "libs/cua-driver"
SKILL_SOURCE = DRIVER_ROOT / "rust/Skills/cua-driver"
PLUGIN_ROOT = DRIVER_ROOT / "plugins/cua-driver"
VERSION_FILE = DRIVER_ROOT / "rust/VERSION"

SKILL_FILES = (
    "SKILL.md",
    "MACOS.md",
    "WINDOWS.md",
    "LINUX.md",
    "BROWSER.md",
    "RECORDING.md",
    "EMBEDDING.md",
)

DESCRIPTION = (
    "Drive native macOS, Windows, and Linux applications through Cua Driver "
    "with snapshot-bound, verified UI actions."
)
SHORT_DESCRIPTION = "Drive native apps with verified Cua Driver actions."
LONG_DESCRIPTION = (
    "Connects an agent to the locally installed Cua Driver MCP server and "
    "teaches the snapshot, action, and verification workflow for safe desktop "
    "automation across macOS, Windows, and Linux."
)

PLUGIN_SKILL_DESCRIPTION = (
    "Use the bundled Cua Driver MCP server only when explicitly invoked as "
    "`$cua-driver` or selected through the Cua Driver plugin. Do not use for "
    "generic or plain-language computer-use requests."
)

PLUGIN_TRANSPORT_NOTICE = """> **Plugin transport rule:** Use the bundled `cua-driver` MCP tools for every operation in this file.
> Treat any `cua-driver ...` shell command as standalone documentation only; do not execute it from the plugin.
> If a matching MCP tool is unavailable, stop instead of falling back to a shell command or another provider.

"""

SKILL_BODY_START = """---

# cua-driver
"""
PLUGIN_SKILL_BODY_START = f"""---

{PLUGIN_TRANSPORT_NOTICE}# cua-driver
"""

GENERIC_SKILL_INTRO = """Orchestrates cross-platform app automation via `cua-driver`. Whenever
a user asks to drive a native app, follow the loop in this skill
rather than calling tools ad-hoc — the snapshot-before-action
invariant is not optional and silently breaks if you skip it.
"""
PLUGIN_SKILL_INTRO = """Orchestrates cross-platform app automation via the bundled `cua-driver`
MCP server. After this Skill has been explicitly invoked, follow the loop below
rather than calling tools ad-hoc — the snapshot-before-action invariant is not
optional and silently breaks if you skip it.
"""

GENERIC_GUI_ROUTING_BLOCK = """Use Cua Driver when the outcome lives in an application's UI or window state,
or when the user explicitly asks to operate that GUI. Once the task crosses
that boundary, do not replace Cua's targeted and verified actions with shell
scripts that mutate the app UI. A shell is a capability of the calling agent,
not of the Cua Driver MCP server; an MCP-only client must not assume one exists.
"""
PLUGIN_GUI_ROUTING_BLOCK = """After this Skill has been explicitly invoked, use Cua Driver for outcomes in
an application's UI or window state. Do not treat a generic GUI request as
authorization to load or route through this Skill. Do not replace Cua's
targeted and verified actions with shell scripts that mutate the app UI.
"""

GENERIC_WINDOWS_INTRO = """Orchestrates Windows app automation via the `cua-driver` binary (`cua-driver.exe`). Whenever a user
asks to drive a native Windows app, follow the loop in this doc
rather than calling tools ad-hoc — the snapshot-before-action
invariant is not optional and silently breaks if you skip it.
"""
PLUGIN_WINDOWS_INTRO = """Extends an explicitly invoked Cua Driver plugin task with Windows-specific
automation guidance. Follow the loop in this document rather than calling tools
ad-hoc — the snapshot-before-action invariant is not optional and silently
breaks if you skip it.
"""

CLI_TRANSPORT_BLOCK = """## GUI transport defaults — prefer cua-driver over GUI shell shims

**Default transport is the `cua-driver` CLI** — `Bash` shelling out
to `cua-driver <tool-name> '<JSON-args>'`. MCP tools (prefix
`mcp__cua-driver__*`) only when the user explicitly asks for them.
CLI wins because it picks up rebuilds instantly, failures are
easier to diagnose, and there's no per-tool schema-load overhead.

Every reference to `click(...)`, `get_window_state(...)` etc. in this
skill means `cua-driver click '{...}'` — translate to MCP form only
when MCP is requested.
"""

PLUGIN_MCP_TRANSPORT_BLOCK = """## GUI transport defaults — use the bundled MCP server

**Default transport is the bundled `cua-driver` MCP server.** Use its exposed
tools for every Cua Driver operation in this skill.
Do not shell out to `cua-driver`.

If the bundled MCP tools are unavailable or the server cannot start, report the
exact installation or connection failure and stop. Do not silently fall back
to the host's built-in computer-use provider (including Codex Computer Use or
`@oai/sky`), AppleScript, another MCP server, or any other provider.
"""

CLAUDE_DEFAULT_TRANSPORT_REFERENCE = """For normal Claude Code use, keep the default CLI or `cua-driver` MCP
server path above. If the user explicitly wants Claude Code's
vision/computer-use-style flow, they can register:
"""
PLUGIN_CLAUDE_TRANSPORT_REFERENCE = """For normal Claude Code plugin use, keep the bundled `cua-driver` MCP server
path above. The compatibility registration command below is standalone setup
for users configuring Claude Code outside the plugin:
"""

WINDOWS_CLI_TRANSPORT_BLOCK = """## Defaults — always prefer cua-driver over shell shims

**Default transport is the `cua-driver` CLI** — `Bash` shelling out
to `cua-driver <tool-name>` with JSON piped via stdin (avoids
PowerShell 5.1's argv quoting quirks for strings containing both
quotes and spaces). MCP tools (prefix `mcp__cua-driver__*`) only when
the user explicitly asks for them. CLI wins because it picks up
rebuilds instantly, failures are easier to diagnose, and there's no
per-tool schema-load overhead.

Every reference to `click(...)`, `get_window_state(...)` etc. in this
doc means `cua-driver <name>` with JSON piped via stdin — translate
to MCP form only when MCP is requested.
"""

WINDOWS_PLUGIN_MCP_TRANSPORT_BLOCK = """## Defaults — use the bundled MCP server

The plugin's default transport is the bundled `cua-driver` MCP server. Use its
exposed tools for every Cua Driver operation in this document. Do not shell out
to `cua-driver`, and apply the same no-fallback rule from `SKILL.md`.
"""

WINDOWS_CLI_ARGUMENT_HEADING = """### CLI argument plumbing on Windows

"""
WINDOWS_PLUGIN_CLI_ARGUMENT_HEADING = """### Standalone CLI argument reference

The forms below are for users configuring Cua Driver outside the plugin.
Plugin workflows must keep using the bundled MCP tools.

"""

SHELL_REFERENCE_HEADING = """## Using cua-driver from the shell

"""
PLUGIN_SHELL_REFERENCE_HEADING = """## Standalone CLI reference — not for plugin execution

The commands below document standalone Cua Driver use. Plugin workflows must
use the bundled MCP tools and must not execute these shell forms.

"""

ACTION_RESULT_REFERENCE = """The full wire contract and 0.14 migration notes are in
`../../../docs/action-result-contract.md`.
"""
PORTABLE_ACTION_RESULT_REFERENCE = """The full wire contract and 0.14 migration notes are in
[`action-result-contract.md`](action-result-contract.md).
"""

OPENAI_SKILL_CONFIG = """interface:
  display_name: "Cua Driver"
  short_description: "Drive native apps through bundled Cua Driver MCP"
  default_prompt: "Use $cua-driver to inspect the selected app without interacting yet."
policy:
  allow_implicit_invocation: false
"""

WINDOWS_INSTALL_BLOCK = """   If missing, point the user at:
   ```powershell
   irm https://cua.ai/driver/install.ps1 | iex
   ```
   and stop.
"""
WINDOWS_PORTABLE_INSTALL_BLOCK = """   If missing, direct the user to the Cua Driver installation guide at
   `https://cua.ai/docs/how-to-guides/driver/install` and stop. The plugin must
   not download or execute the native installer.
"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail when the checked-in plugin package differs from generated output.",
    )
    return parser.parse_args()


def read_version() -> str:
    version = VERSION_FILE.read_text(encoding="utf-8").strip()
    if re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", version) is None:
        raise SystemExit(f"invalid Cua Driver version in {VERSION_FILE}: {version!r}")
    return version


def write_json(path: Path, payload: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def common_manifest(version: str) -> dict[str, object]:
    return {
        "name": "cua-driver",
        "version": version,
        "description": DESCRIPTION,
        "author": {
            "name": "Cua AI, Inc.",
            "url": "https://cua.ai",
        },
        "homepage": "https://cua.ai/docs/cua-driver",
        "repository": "https://github.com/trycua/cua",
        "license": "MIT",
        "keywords": ["cua", "cua driver", "cua-driver", "computer use"],
    }


def portable_skill_contents(filename: str) -> str:
    source = SKILL_SOURCE / filename
    contents = source.read_text(encoding="utf-8")
    if filename == "SKILL.md":
        contents, replacements = re.subn(
            r"(?m)^version: .* # x-release-please-version\n",
            "",
            contents,
        )
        if replacements != 1:
            raise SystemExit(f"expected one release version in {source}")
        contents, replacements = re.subn(
            r"(?m)^description: .*\n",
            f"description: {PLUGIN_SKILL_DESCRIPTION}\n",
            contents,
        )
        if replacements != 1:
            raise SystemExit(f"expected one skill description in {source}")
        if contents.count(SKILL_BODY_START) != 1:
            raise SystemExit(f"expected one skill body start in {source}")
        contents = contents.replace(
            SKILL_BODY_START,
            PLUGIN_SKILL_BODY_START,
        )
        if contents.count(GENERIC_SKILL_INTRO) != 1:
            raise SystemExit(f"expected one generic skill intro in {source}")
        contents = contents.replace(GENERIC_SKILL_INTRO, PLUGIN_SKILL_INTRO)
        if contents.count(GENERIC_GUI_ROUTING_BLOCK) != 1:
            raise SystemExit(f"expected one generic GUI routing block in {source}")
        contents = contents.replace(
            GENERIC_GUI_ROUTING_BLOCK,
            PLUGIN_GUI_ROUTING_BLOCK,
        )
        if contents.count(CLI_TRANSPORT_BLOCK) != 1:
            raise SystemExit(f"expected one CLI transport block in {source}")
        contents = contents.replace(
            CLI_TRANSPORT_BLOCK,
            PLUGIN_MCP_TRANSPORT_BLOCK,
        )
        if contents.count(ACTION_RESULT_REFERENCE) != 1:
            raise SystemExit(f"expected one action-result reference in {source}")
        contents = contents.replace(
            ACTION_RESULT_REFERENCE,
            PORTABLE_ACTION_RESULT_REFERENCE,
        )
        if contents.count(CLAUDE_DEFAULT_TRANSPORT_REFERENCE) != 1:
            raise SystemExit(f"expected one Claude transport reference in {source}")
        contents = contents.replace(
            CLAUDE_DEFAULT_TRANSPORT_REFERENCE,
            PLUGIN_CLAUDE_TRANSPORT_REFERENCE,
        )
    if filename == "WINDOWS.md":
        if contents.count(GENERIC_WINDOWS_INTRO) != 1:
            raise SystemExit(f"expected one generic Windows intro in {source}")
        contents = contents.replace(GENERIC_WINDOWS_INTRO, PLUGIN_WINDOWS_INTRO)
        if contents.count(WINDOWS_INSTALL_BLOCK) != 1:
            raise SystemExit(f"expected one installer block in {source}")
        contents = contents.replace(
            WINDOWS_INSTALL_BLOCK,
            WINDOWS_PORTABLE_INSTALL_BLOCK,
        )
        if contents.count(WINDOWS_CLI_TRANSPORT_BLOCK) != 1:
            raise SystemExit(f"expected one Windows CLI transport block in {source}")
        contents = contents.replace(
            WINDOWS_CLI_TRANSPORT_BLOCK,
            WINDOWS_PLUGIN_MCP_TRANSPORT_BLOCK,
        )
        if contents.count(WINDOWS_CLI_ARGUMENT_HEADING) != 1:
            raise SystemExit(f"expected one Windows CLI argument heading in {source}")
        contents = contents.replace(
            WINDOWS_CLI_ARGUMENT_HEADING,
            WINDOWS_PLUGIN_CLI_ARGUMENT_HEADING,
        )
    if filename in {"SKILL.md", "WINDOWS.md"}:
        if contents.count(SHELL_REFERENCE_HEADING) != 1:
            raise SystemExit(f"expected one shell reference heading in {source}")
        contents = contents.replace(
            SHELL_REFERENCE_HEADING,
            PLUGIN_SHELL_REFERENCE_HEADING,
        )
    if filename != "SKILL.md":
        contents = PLUGIN_TRANSPORT_NOTICE + contents
    return contents


def render_package(destination: Path) -> None:
    version = read_version()
    skill_contents = {filename: portable_skill_contents(filename) for filename in SKILL_FILES}
    action_result_contract = (DRIVER_ROOT / "docs/action-result-contract.md").read_bytes()

    # 插件目录除手写 README 外均由生成器拥有；先清空，避免重命名后残留旧文件。
    if destination.exists():
        for child in destination.iterdir():
            if child.name == "README.md":
                continue
            if child.is_dir() and not child.is_symlink():
                shutil.rmtree(child)
            else:
                child.unlink()

    skill_destination = destination / "skills/cua-driver"
    skill_destination.mkdir(parents=True, exist_ok=True)
    for filename, contents in skill_contents.items():
        target = skill_destination / filename
        target.write_text(contents, encoding="utf-8")
    (skill_destination / "action-result-contract.md").write_bytes(action_result_contract)
    openai_policy = skill_destination / "agents/openai.yaml"
    openai_policy.parent.mkdir(parents=True, exist_ok=True)
    openai_policy.write_text(OPENAI_SKILL_CONFIG, encoding="utf-8")

    mcp_manifest = {
        "mcpServers": {
            "cua-driver": {
                "command": "cua-driver",
                "args": ["mcp"],
            }
        }
    }
    write_json(destination / ".mcp.json", mcp_manifest)

    portable_manifest = common_manifest(version)
    write_json(destination / ".claude-plugin/plugin.json", portable_manifest)
    write_json(destination / ".grok-plugin/plugin.json", portable_manifest)

    codex_manifest = {
        **portable_manifest,
        "skills": "./skills/",
        "mcpServers": "./.mcp.json",
        "interface": {
            "displayName": "Cua Driver",
            "shortDescription": SHORT_DESCRIPTION,
            "longDescription": LONG_DESCRIPTION,
            "developerName": "Cua AI, Inc.",
            "category": "Developer Tools",
            "capabilities": ["Interactive", "Write"],
            "websiteURL": "https://cua.ai",
            "privacyPolicyURL": "https://cua.ai/privacy-policy",
            "termsOfServiceURL": "https://cua.ai/terms-of-service",
            "brandColor": "#20C997",
            "defaultPrompt": [
                "List the visible apps and windows without interacting with them.",
                "Inspect the selected app and explain what is visible.",
                "Complete this desktop task and verify each action from fresh state.",
            ],
        },
    }
    write_json(destination / ".codex-plugin/plugin.json", codex_manifest)


def files_under(root: Path) -> dict[str, bytes]:
    if not root.exists():
        return {}
    return {
        path.relative_to(root).as_posix(): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file() and path != root / "README.md"
    }


def check_package() -> int:
    with tempfile.TemporaryDirectory(prefix="cua-driver-plugin-") as temp_dir:
        generated_root = Path(temp_dir) / "cua-driver"
        render_package(generated_root)
        expected = files_under(generated_root)
        actual = files_under(PLUGIN_ROOT)
    if actual == expected:
        print("Cua Driver agent plugin package is up to date.")
        return 0

    missing = sorted(expected.keys() - actual.keys())
    extra = sorted(actual.keys() - expected.keys())
    changed = sorted(
        path for path in expected.keys() & actual.keys() if expected[path] != actual[path]
    )
    for label, paths in (("missing", missing), ("extra", extra), ("changed", changed)):
        for path in paths:
            print(f"{label}: {path}", file=sys.stderr)
    print(
        "Run `python3 libs/cua-driver/scripts/generate-agent-plugin.py`.",
        file=sys.stderr,
    )
    return 1


def main() -> None:
    args = parse_args()
    if args.check:
        raise SystemExit(check_package())
    render_package(PLUGIN_ROOT)
    print(f"Generated Cua Driver agent plugin: {PLUGIN_ROOT}")


if __name__ == "__main__":
    main()
