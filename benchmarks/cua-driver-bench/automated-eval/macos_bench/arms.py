"""Isolated Codex CLI homes for the two comparison arms.

Both arms run the same Codex CLI binary, the same model route, the same
reasoning effort, the same shell sandbox and the same unattended preamble.
Only the computer-use tool layer differs:

* ``cua-driver-mcp``: the Cua Driver MCP server (`cua-driver mcp`) plus the Cua
  Driver skill that ships with that release.
* ``codex-native-cu``: Codex's own computer use (the `node_repl` MCP server of the
  ChatGPT/Codex app with the `@oai/sky` service) plus the computer-use skill and
  API reference that ship in the same app bundle.

Nothing here reads credentials. The model route is the local LiteLLM gateway
already configured for Codex (see `provider_block`).
"""

from __future__ import annotations

import json
import os
import shutil
from dataclasses import dataclass
from pathlib import Path

ARMS = ("cua-driver-mcp", "codex-native-cu")  # the Codex-CLI pilot arms (run_pilot.py)
# Claude Code arms (run_bench.py): same model, same harness, only the MCP tools differ.
CLAUDE_ARMS = ("cc-cua-driver", "cc-codex-cu")
# Fallback arms, `codex exec --json`, behind --allow-codex-arms in run_bench.py.
CODEX_FALLBACK_ARMS = ("codex-native-cu", "codex-cua-driver")
ARM_ALIASES = {"codex-cua-driver": "cua-driver-mcp"}  # name used by the pilot code paths
ALL_ARMS = CLAUDE_ARMS + CODEX_FALLBACK_ARMS

CODEX_APP_RESOURCES = Path("/Applications/ChatGPT.app/Contents/Resources")
CODEX_CU_DIR = Path.home() / ".codex" / "computer-use"
CODEX_CU_APP = CODEX_CU_DIR / "Codex Computer Use.app"
CODEX_CU_CLIENT = (
    CODEX_CU_APP
    / "Contents/SharedSupport/SkyComputerUseClient.app/Contents/MacOS/SkyComputerUseClient"
)
CODEX_CU_PLUGIN = CODEX_APP_RESOURCES / "plugins/openai-bundled/plugins/computer-use"
CUA_DRIVER_BIN = Path.home() / ".local" / "bin" / "cua-driver"
CUA_DRIVER_SKILL = Path.home() / ".agents" / "skills" / "cua-driver"
CUA_DRIVER_SOCKET: str | None = None  # private daemon socket; None = the default daemon


def use_cua_release(binary: Path, skill_dir: Path, socket: str | None) -> None:
    """Select the Cua Driver release under test (binary, matching skill, daemon socket)."""
    global CUA_DRIVER_BIN, CUA_DRIVER_SKILL, CUA_DRIVER_SOCKET
    CUA_DRIVER_BIN, CUA_DRIVER_SKILL, CUA_DRIVER_SOCKET = binary, skill_dir, socket


CODEX_BIN = Path.home() / ".local" / "bin" / "codex"

CLOSED_PATH = "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"

PREAMBLE = """\
This is an automated, unattended benchmark run in a disposable environment with synthetic data.
No human is available: do not ask for confirmation or clarification, and treat every action the task requires as pre-approved.
Operate only the applications the task names. Do not open, read or change any other application, window or file.
Work until the task is complete, then reply with a short summary of what you did.

"""


@dataclass(frozen=True)
class ArmSpec:
    name: str
    description: str


ARM_SPECS = {
    "cua-driver-mcp": ArmSpec("cua-driver-mcp", "Codex CLI + Cua Driver MCP + Cua Driver skill"),
    "codex-native-cu": ArmSpec(
        "codex-native-cu",
        "Codex CLI + Codex computer use (node_repl + @oai/sky) + Codex computer-use skill",
    ),
}


def _q(value: str) -> str:
    return json.dumps(value, ensure_ascii=True)


def _sh(value: str) -> str:
    return "'" + value.replace("'", "'\\''") + "'"


def provider_block() -> str:
    """The LiteLLM provider block used for every arm (identical text)."""
    return (
        "[model_providers.litellm]\n"
        'name = "LiteLLM"\n'
        'base_url = "http://127.0.0.1:4000/v1"\n'
        'wire_api = "responses"\n'
        "requires_openai_auth = false\n"
        "request_max_retries = 2\n"
        "stream_max_retries = 2\n"
        "stream_idle_timeout_ms = 120000\n"
    )


def common_config(model: str, effort: str) -> str:
    return (
        f"model = {_q(model)}\n"
        'model_provider = "litellm"\n'
        f"model_reasoning_effort = {_q(effort)}\n"
        'approval_policy = "never"\n'
        'sandbox_mode = "workspace-write"\n'
        "mcp_optional_startup_grace_ms = 30000\n"
        "\n"
        "[sandbox_workspace_write]\n"
        "network_access = true\n"
        "\n" + provider_block()
    )


def cua_driver_config(command: Path) -> str:
    return (
        "\n[mcp_servers.cua]\n"
        f"command = {_q(str(command))}\n"
        'args = ["mcp"]\n'
        "startup_timeout_sec = 60\n"
        'default_tools_approval_mode = "approve"\n'
    )


def codex_cu_config() -> str:
    node_dir = CODEX_APP_RESOURCES / "cua_node"
    env = {
        "NODE_REPL_NATIVE_PIPE_CONNECT_TIMEOUT_MS": "1000",
        "NODE_REPL_NODE_MODULE_DIRS": str(node_dir / "lib/node_modules"),
        "NODE_REPL_NODE_PATH": str(node_dir / "bin/node"),
        "NODE_REPL_TRUSTED_CODE_PATHS": str(node_dir / "lib/node_modules"),
        "NODE_REPL_TRUSTED_SERVICES": json.dumps({"sky": "@oai/sky/service"}),
        "SKY_CUA_SERVICE_PATH": str(CODEX_CU_APP),
    }
    lines = [
        "",
        f'notify = [{_q(str(CODEX_CU_CLIENT))}, "turn-ended"]',
        "",
        "[mcp_servers.node_repl]",
        f"command = {_q(str(node_dir / 'bin/node_repl'))}",
        "args = []",
        "startup_timeout_sec = 120",
        'default_tools_approval_mode = "approve"',
        "",
        "[mcp_servers.node_repl.env]",
    ]
    lines += [f"{key} = {_q(value)}" for key, value in env.items()]
    return "\n".join(lines) + "\n"


def render_home(arm: str, home: Path, model: str, effort: str) -> dict[str, str]:
    """Write config and skill files for ``arm`` under ``home``; return the env."""
    arm = ARM_ALIASES.get(arm, arm)
    if arm not in ARMS:
        raise ValueError(f"unknown arm: {arm}")
    codex_home = home / ".codex"
    codex_home.mkdir(parents=True, exist_ok=True)
    config = common_config(model, effort)
    skills = home / ".agents" / "skills"
    path = CLOSED_PATH
    if arm == "cua-driver-mcp":
        command = CUA_DRIVER_BIN
        if CUA_DRIVER_SOCKET:
            # A wrapper named `cua-driver` so both the MCP server and the CLI the skill
            # recommends reach the release under test through its private daemon socket.
            bin_dir = home / "bin"
            bin_dir.mkdir(parents=True, exist_ok=True)
            wrapper = bin_dir / "cua-driver"
            wrapper.write_text(
                f'#!/bin/sh\nexec {_sh(str(CUA_DRIVER_BIN))} --socket {_sh(CUA_DRIVER_SOCKET)} "$@"\n',
                "utf-8",
            )
            wrapper.chmod(0o755)
            command = wrapper
            path = f"{bin_dir}:{CLOSED_PATH}"
        config += cua_driver_config(command)
        shutil.copytree(CUA_DRIVER_SKILL, skills / "cua-driver")
    else:
        config += codex_cu_config()
        target = skills / "computer-use"
        target.mkdir(parents=True, exist_ok=True)
        skill = (CODEX_CU_PLUGIN / "skills/computer-use/SKILL.md").read_text("utf-8")
        api = (CODEX_CU_PLUGIN / ".codex-plugin/computer-use-node-repl.md").read_text("utf-8")
        (target / "SKILL.md").write_text(skill.rstrip() + "\n\n" + api, "utf-8")
    (codex_home / "config.toml").write_text(config, "utf-8")
    env = {
        "HOME": str(home),
        "CODEX_HOME": str(codex_home),
        "PATH": path,
        "TMPDIR": str(home / "tmp"),
    }
    (home / "tmp").mkdir(exist_ok=True)
    gateway_key = os.environ.get("LITELLM_MASTER_KEY")
    if gateway_key and os.environ.get("CDB_PILOT_PASS_GATEWAY_KEY") == "1":
        env["LITELLM_MASTER_KEY"] = gateway_key
    return env


def codex_argv(workspace: Path, model: str) -> list[str]:
    return [
        str(CODEX_BIN),
        "exec",
        "--json",
        "--skip-git-repo-check",
        "-s",
        "workspace-write",
        "-C",
        str(workspace),
        "-m",
        model,
        "-",
    ]


def preflight_arm(arm: str) -> list[str]:
    """Static checks that an arm's binaries exist. Returns a list of problems."""
    arm = ARM_ALIASES.get(arm, arm)
    problems: list[str] = []
    if not CODEX_BIN.is_file():
        problems.append(f"Codex CLI missing: {CODEX_BIN}")
    if arm == "cua-driver-mcp":
        if not CUA_DRIVER_BIN.exists():
            problems.append(f"cua-driver missing: {CUA_DRIVER_BIN}")
        if not (CUA_DRIVER_SKILL / "SKILL.md").is_file():
            problems.append("cua-driver skill missing")
    else:
        for path in (
            CODEX_APP_RESOURCES / "cua_node/bin/node_repl",
            CODEX_CU_APP,
            CODEX_CU_PLUGIN / "skills/computer-use/SKILL.md",
            CODEX_CU_PLUGIN / ".codex-plugin/computer-use-node-repl.md",
        ):
            if not path.exists():
                problems.append(f"Codex computer use component missing: {path}")
    return problems
