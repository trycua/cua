"""Claude Code arms: one identical `claude -p` invocation, only the MCP server (and, for arm A, the
skill) differs.

* ``cc-cua-driver``: Cua Driver 0.34.0 (official release, private app + private daemon) through its
  MCP server, plus the Cua Driver skill that ships in the same release, delivered through Claude
  Code's own project-skill mechanism (``<cwd>/.claude/skills/cua-driver``).
* ``cc-cua-driver-main``: the same, with Cua Driver built from a pinned main commit (Amendment 3,
  7 Oct 2026) and the skill of that commit. Its own private app, daemon, socket and state.
* ``cc-cua-driver-main-skill``: the main build of ``cc-cua-driver-main`` with the text of its ``SKILL.md``
  appended to the system prompt (Amendment 4, CUA-1226). The project skill stays in place, so the skill's other
  files can still be read; only the system prompt differs from ``cc-cua-driver-main``.
* ``cc-codex-cu``: OpenAI's Codex computer-use ``cua_repl`` launcher as MCP server ``codex-cu``
  (configuration supplied by the codex-access worker, used unmodified). No skill.

Nothing here reads credentials. ``claude`` keeps using the existing OAuth login because HOME is
left alone; everything else that could leak (CLAUDE_CODE_* variables, user settings, user skills,
plugins, hooks, CLAUDE.md files, auto memory, other MCP servers) is removed.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
# WORK holds everything the harness creates or downloads: the private Cua Driver install, the Codex MCP config,
# the spend ledger, the HOLD file and the run folders. CDB_BENCH_WORK overrides it.
_WORK_ENV = os.environ.get("CDB_BENCH_WORK")
WORK = (
    Path(_WORK_ENV).expanduser()
    if _WORK_ENV
    else (
        HERE.parents[1] if HERE.parent.name == "src" else Path.home() / ".cache" / "cua-bench-h2h"
    )
)

CLAUDE_BIN = Path("/opt/homebrew/bin/claude")
CLAUDE_ENV_ALLOW = ("HOME", "PATH", "USER", "LANG", "TMPDIR", "TERM")
CLAUDE_ENV_FIXED = {
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
    "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1",
    "DISABLE_AUTOUPDATER": "1",
}
CLAUDE_PATH = "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"

SYSTEM_PROMPT = (
    "You are an autonomous computer-use agent operating a macOS desktop through the tools provided. "
    "No human is available to answer questions or confirm actions; every action the task requires is "
    "pre-approved. Operate only the applications the task names. Do not use the network. When the task "
    "is done, reply with the single word DONE. If you cannot finish, reply BLOCKED and the reason."
)

BASE_TOOLS = ("Skill", "Read")
TOOL_SEARCH_TOOL = "ToolSearch"
CODING_TOOLS = ("Bash", "Edit", "Write")  # added to both arms for the CDB tasks only (TASKS.md)
TOKEN_FILE_ENV = "CDB_CLAUDE_TOKEN_FILE"  # optional 0600 file with a CLAUDE_CODE_OAUTH_TOKEN, read by fd

CUA_APP = WORK / "cua-0.34.0" / "CuaDriver-0.34.0.app"
CUA_BIN = CUA_APP / "Contents/MacOS/cua-driver"
CUA_STATE = WORK / "cua-0.34.0" / "daemon-state"
CUA_SKILLS = WORK / "cua-0.34.0" / "skills-ex" / "cua-driver-rs-v0.34.0-skills"
AGENT_SOCKET = "/tmp/cdb-bench-cua-0340.sock"
RECORDER_SOCKET = "/tmp/cdb-bench-cua-rec.sock"
RECORDER_STATE = WORK / "cua-0.34.0" / "recorder-state"
PINS_FILE = HERE / "pins.json"

# Amendment 3: Cua Driver built from main inside the VM (tools/build_cua_main.sh). Ad-hoc signed private app
# with its own bundle id, so its Accessibility and Screen Recording grants are separate from 0.34.0's.
CUA_MAIN_DIR = WORK / "cua-main"
CUA_MAIN_APP = CUA_MAIN_DIR / "CuaDriverBenchMain.app"
MAIN_SOCKET = "/tmp/cdb-bench-cua-main.sock"


@dataclass(frozen=True)
class CuaBuild:
    """One Cua Driver build under test: private app, daemon socket, daemon state and matching skill."""

    arm: str
    label: str
    app: Path
    socket: str
    state: Path
    skills: Path
    skill_in_prompt: bool = False  # Amendment 4: SKILL.md appended to the system prompt

    @property
    def bin(self) -> Path:
        return self.app / "Contents/MacOS/cua-driver"

    @property
    def home(self) -> Path:
        return self.state / "home"


CUA_BUILDS = {
    "cc-cua-driver": CuaBuild("cc-cua-driver", "0.34.0", CUA_APP, AGENT_SOCKET, CUA_STATE, CUA_SKILLS),
    "cc-cua-driver-main": CuaBuild(
        "cc-cua-driver-main",
        "main",
        CUA_MAIN_APP,
        MAIN_SOCKET,
        CUA_MAIN_DIR / "daemon-state",
        CUA_MAIN_DIR / "skills" / "cua-driver",
    ),
    # Amendment 4 (CUA-1226): the same build, daemon and skill as cc-cua-driver-main; SKILL.md in the system prompt.
    "cc-cua-driver-main-skill": CuaBuild(
        "cc-cua-driver-main-skill",
        "main",
        CUA_MAIN_APP,
        MAIN_SOCKET,
        CUA_MAIN_DIR / "daemon-state",
        CUA_MAIN_DIR / "skills" / "cua-driver",
        skill_in_prompt=True,
    ),
}
CUA_ARMS = tuple(CUA_BUILDS)

CODEX_CU_MCP_CANDIDATES = (
    WORK / "codex-access" / "mcp.json",
    WORK / "codex-access" / "mcp_cua_repl_computer.json",
)
CWD_ROOT = Path("/tmp/cdb-bench-cwd/work")  # outside every git checkout, same path for both arms

ARM_DESCRIPTIONS = {
    "cc-cua-driver": "Claude Code + Cua Driver 0.34.0 MCP + Cua Driver skill (0.34.0 release)",
    "cc-cua-driver-main": "Claude Code + Cua Driver built from main (pinned commit) MCP + the skill of that commit",
    "cc-cua-driver-main-skill": "The same as cc-cua-driver-main, with the skill's SKILL.md appended to the system prompt",
    "cc-codex-cu": "Claude Code + Codex computer-use cua_repl MCP (server codex-cu), no skill",
}

SKILL_PROMPT_HEADER = (
    "\n\nThe instructions of the Cua Driver skill follow. They apply to the `cua` MCP server. The skill's other "
    "files, which these instructions refer to, are in .claude/skills/cua-driver/ and can be read with the Read tool."
    "\n\n"
)


def system_prompt_for(arm: str) -> str:
    """The shared SYSTEM_PROMPT; for a skill-in-prompt arm, followed by its build's SKILL.md, verbatim."""
    build = CUA_BUILDS.get(arm)
    if build is None or not build.skill_in_prompt:
        return SYSTEM_PROMPT
    return SYSTEM_PROMPT + SKILL_PROMPT_HEADER + (build.skills / "SKILL.md").read_text("utf-8").strip() + "\n"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_tree(path: Path) -> str:
    digest = hashlib.sha256()
    for item in sorted(p for p in path.rglob("*") if p.is_file()):
        digest.update(str(item.relative_to(path)).encode())
        digest.update(sha256_file(item).encode())
    return digest.hexdigest()


def load_pins() -> dict[str, Any]:
    return json.loads(PINS_FILE.read_text("utf-8"))


def cua_env(home: Path) -> dict[str, str]:
    """Environment of the Cua Driver daemon and of its MCP client: own HOME, telemetry off."""
    return {
        "HOME": str(home),
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
        "DO_NOT_TRACK": "1",  # the main build's shared telemetry crate also honours this (Amendment 3)
    }


def resolve_codex_cu_config(explicit: Path | None = None) -> Path | None:
    if explicit is not None:
        return explicit if explicit.is_file() else None
    for candidate in CODEX_CU_MCP_CANDIDATES:
        if candidate.is_file():
            return candidate
    return None


def mcp_config_for(
    arm: str, run_dir: Path, codex_cu_config: Path | None = None
) -> tuple[Path, str]:
    """Write the arm's ``--mcp-config`` file under ``run_dir`` and return (path, server name)."""
    run_dir.mkdir(parents=True, exist_ok=True)
    if arm in CUA_BUILDS:
        build = CUA_BUILDS[arm]
        config = {
            "mcpServers": {
                "cua": {
                    "type": "stdio",
                    "command": str(build.bin),  # absolute: no PATH lookup can find another copy
                    "args": ["--socket", build.socket, "mcp"],
                    "env": cua_env(build.home),
                }
            }
        }
        path = run_dir / f"mcp-{arm}.json"
        path.write_text(json.dumps(config, indent=2) + "\n", "utf-8")
        return path, "cua"
    if arm == "cc-codex-cu":
        source = resolve_codex_cu_config(codex_cu_config)
        if source is None:
            raise FileNotFoundError(
                "Codex computer-use MCP config not found (WORK/codex-access/mcp.json); arm cc-codex-cu is disabled"
            )
        data = json.loads(source.read_text("utf-8"))
        servers = list((data.get("mcpServers") or {}))
        if len(servers) != 1:
            raise ValueError(f"{source}: expected exactly one MCP server, found {servers}")
        path = run_dir / "mcp-cc-codex-cu.json"
        shutil.copyfile(source, path)  # unmodified copy; the run keeps its own record
        return path, servers[0]
    raise ValueError(f"not a Claude arm: {arm}")


def prepare_cwd(arm: str, cwd: Path = CWD_ROOT) -> Path:
    """Fresh working directory. Only the Cua Driver arms get ``.claude/skills/cua-driver`` (their build's)."""
    if cwd.exists():
        shutil.rmtree(cwd)
    cwd.mkdir(parents=True)
    if arm in CUA_BUILDS:
        target = cwd / ".claude" / "skills" / "cua-driver"
        target.parent.mkdir(parents=True)
        # The module-level CUA_SKILLS stays the source for 0.34.0 so tests can point it elsewhere.
        shutil.copytree(CUA_SKILLS if arm == "cc-cua-driver" else CUA_BUILDS[arm].skills, target)
    return cwd


def claude_env() -> dict[str, str]:
    """Scrubbed environment: allowlist only, so CLAUDE_CODE_* variables of a parent session cannot leak."""
    env = {key: os.environ[key] for key in CLAUDE_ENV_ALLOW if key in os.environ}
    env["PATH"] = CLAUDE_PATH
    env.setdefault("LANG", "en_US.UTF-8")
    env.setdefault("TERM", "xterm-256color")
    env.update(CLAUDE_ENV_FIXED)
    return env


def open_token_fd() -> int | None:
    """A fresh inheritable fd on the OAuth token file (None when unset). The token never enters an
    environment variable, an argv or a log: claude reads it from the descriptor named by
    CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR. Every spawn needs its own fd (the offset is shared)."""
    path = os.environ.get(TOKEN_FILE_ENV)
    if not path:
        return None
    fd = os.open(path, os.O_RDONLY)
    os.set_inheritable(fd, True)
    return fd


def builtin_tools(tool_search: bool = True, coding: bool = False) -> list[str]:
    tools = list(BASE_TOOLS)
    if coding:
        tools.extend(CODING_TOOLS)
    if tool_search:
        tools.append(TOOL_SEARCH_TOOL)
    return tools


def claude_argv(
    *,
    mcp_config: Path,
    server: str | None,
    model: str,
    max_turns: int,
    max_budget_usd: float,
    tool_search: bool = True,
    effort: str | None = None,
    system_prompt: str = SYSTEM_PROMPT,
    claude_bin: Path | None = None,
    debug_file: Path | None = None,
    coding_tools: bool = False,
) -> list[str]:
    """The one invocation, identical for both arms apart from the MCP config and its server name."""
    argv = [
        str(claude_bin or CLAUDE_BIN),  # resolved at call time so tests can substitute the binary
        "-p",
        "--model",
        model,
        "--system-prompt",
        system_prompt,
        "--strict-mcp-config",
        "--mcp-config",
        str(mcp_config),
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--tools",
        ",".join(builtin_tools(tool_search, coding_tools)),
        "--setting-sources",
        "project",
        "--permission-mode",
        "dontAsk",
        "--max-turns",
        str(max_turns),
        "--max-budget-usd",
        f"{max_budget_usd:g}",
    ]
    if server:
        allowed = [f"mcp__{server}"]
        if coding_tools:
            allowed += [*CODING_TOOLS, "Read"]
        argv += ["--allowedTools", ",".join(allowed)]
    if effort:
        argv += ["--effort", effort]
    if debug_file is not None:
        argv += ["--debug-file", str(debug_file)]
    return argv


# ---------------------------------------------------------------- Cua Driver private daemon


def _run(
    cmd: list[str], env: dict[str, str] | None = None, timeout: float = 20
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=timeout)


def cua_cli(
    *args: str,
    socket: str = AGENT_SOCKET,
    home: Path | None = None,
    timeout: float = 20,
    binary: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    env = cua_env(home or (CUA_STATE / "home"))
    return _run([str(binary or CUA_BIN), "--socket", socket, *args], env=env, timeout=timeout)


def cua_version_string(binary: Path | None = None) -> str:
    try:
        return _run(
            [str(binary or CUA_BIN), "--version"], env={"PATH": "/usr/bin:/bin"}
        ).stdout.strip()
    except (OSError, subprocess.TimeoutExpired) as error:
        return f"unavailable: {error}"


def cua_health(
    socket: str = AGENT_SOCKET, home: Path | None = None, binary: Path | None = None
) -> dict[str, Any]:
    """Daemon-reported build (version, exe sha256, git sha) from the live daemon via health_report."""
    try:
        done = cua_cli(
            "call", "health_report", "{}", socket=socket, home=home, timeout=30, binary=binary
        )
        data = json.loads(done.stdout[done.stdout.index("{") :])
        checks = {c.get("name"): c.get("status") for c in data.get("checks", [])}
        return {"ok": done.returncode == 0, "build": data.get("build", {}), "checks": checks}
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        return {"ok": False, "error": f"{type(error).__name__}: {error}"}


def stop_cua_daemon(socket: str, home: Path, binary: Path | None = None) -> None:
    """Ask a running private daemon on ``socket`` to exit (no-op when none runs)."""
    try:
        _run([str(binary or CUA_BIN), "stop", "--socket", socket], env=cua_env(home), timeout=15)
    except (OSError, subprocess.TimeoutExpired):
        pass
    time.sleep(0.5)


def start_cua_daemon(
    socket: str,
    state_dir: Path,
    overlay: bool = True,
    log_name: str = "daemon.log",
    binary: Path | None = None,
) -> subprocess.Popen[bytes]:
    """Private daemon of the release under test: own socket, own HOME, telemetry off."""
    binary = binary or CUA_BIN
    home = state_dir / "home"
    home.mkdir(parents=True, exist_ok=True)
    try:
        os.unlink(socket)
    except OSError:
        pass
    args = [str(binary), "serve", "--socket", socket, "--dangerously-bypass-approvals"]
    if not overlay:
        args.append("--no-overlay")
    log = (state_dir / log_name).open("ab")
    proc = subprocess.Popen(
        args,
        env=cua_env(home),
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(
                f"Cua Driver daemon exited early (rc={proc.returncode}); see {state_dir / log_name}"
            )
        done = _run([str(binary), "status", "--socket", socket], env=cua_env(home), timeout=10)
        if done.returncode == 0:
            return proc
        time.sleep(0.5)
    raise RuntimeError("Cua Driver daemon did not become ready")


# ---------------------------------------------------------------- pins (observed values)

CHATGPT_APP = Path("/Applications/ChatGPT.app")
UNIFIED_CU_PLUGIN = (
    CHATGPT_APP
    / "Contents/Resources/plugins/openai-bundled/plugins/unified-computer-use/.codex-plugin/plugin.json"
)
CUA_REPL_PACKAGE = (
    CHATGPT_APP / "Contents/Resources/cua_node/lib/node_modules/@oai/cua-repl/package.json"
)
CODEX_CU_SERVICE_PLIST = (
    Path.home() / ".codex/computer-use/Codex Computer Use.app/Contents/Info.plist"
)
CUA_DOWNLOADS = WORK / "cua-0.34.0" / "dl"


def _plist_version(path: Path, key: str = "CFBundleShortVersionString") -> str | None:
    import plistlib

    try:
        return str(plistlib.loads(path.read_bytes()).get(key))
    except (OSError, ValueError):
        return None


def _json_version(path: Path) -> str | None:
    try:
        return str(json.loads(path.read_text("utf-8")).get("version"))
    except (OSError, ValueError):
        return None


def observed_pins(include_codex: bool = True, include_cua: bool = True) -> dict[str, Any]:
    """What is installed right now, keyed like pins.json."""
    out: dict[str, Any] = {}
    claude = (
        _run([str(CLAUDE_BIN), "--version"], env=claude_env(), timeout=30).stdout.strip()
        if CLAUDE_BIN.is_file()
        else None
    )
    out["claude_code_version"] = claude
    out["macos_version"] = _run(["sw_vers", "-productVersion"]).stdout.strip()
    out["macos_build"] = _run(["sw_vers", "-buildVersion"]).stdout.strip()
    if include_cua:
        out["cua_driver_version_string"] = cua_version_string()
        out["cua_driver_version"] = out["cua_driver_version_string"].replace("cua-driver ", "")
        out["cua_driver_binary_sha256"] = sha256_file(CUA_BIN) if CUA_BIN.is_file() else None
        pins = load_pins()
        tar = CUA_DOWNLOADS / pins.get("cua_driver_tarball", "")
        skills_tar = CUA_DOWNLOADS / pins.get("cua_skills_tarball", "")
        out["cua_driver_tarball_sha256"] = sha256_file(tar) if tar.is_file() else None
        out["cua_skills_tarball_sha256"] = sha256_file(skills_tar) if skills_tar.is_file() else None
        out["cua_skills_tree_sha256"] = sha256_tree(CUA_SKILLS) if CUA_SKILLS.is_dir() else None
    if include_codex:
        out["chatgpt_app_version"] = _plist_version(CHATGPT_APP / "Contents/Info.plist")
        out["unified_computer_use_plugin_version"] = _json_version(UNIFIED_CU_PLUGIN)
        out["cua_repl_package_version"] = _json_version(CUA_REPL_PACKAGE)
        out["codex_computer_use_service_version"] = _plist_version(CODEX_CU_SERVICE_PLIST)
    return out
