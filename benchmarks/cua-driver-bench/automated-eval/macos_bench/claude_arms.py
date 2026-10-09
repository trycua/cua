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
# Amendment 6 (CUA-1214): the same main binary in its own app copy and bundle id, with the experimental
# run_script tool switched on in the daemon (CUA_DRIVER_EXPERIMENTAL_SCRIPT=1; the registry is built in `serve`).
CUA_SCRIPT_DIR = WORK / "cua-script"
CUA_SCRIPT_APP = CUA_SCRIPT_DIR / "CuaDriverBenchScript.app"
SCRIPT_SOCKET = "/tmp/cdb-bench-cua-script.sock"
SCRIPT_FLAG = ("CUA_DRIVER_EXPERIMENTAL_SCRIPT", "1")
RUN_SCRIPT_ADDENDUM = (
    "\n\nFor any task with more than one or two actions, write ONE run_script call: a JavaScript async function "
    "body that uses `cua.getApp(\"<App>\")` and app.click/typeText/setValue/pressKey/scroll with {role, name} "
    "targets, app.waitFor({text}) between screens, and `return`s what you need to check the result. Use loops and "
    "if/else instead of separate calls. A failed call throws with the script line and the nearest elements; fix the "
    "script and run it again from the step that failed. Use run_actions or single tools only for one-off actions.\n"
)
# From mini-run v037b (A6.5): run_script's wall-time limit is a per-call argument (default 30 s, max 120 s); v037-full
# saw 14 script timeouts, so the AX addendum asks for the maximum on every call. The driver binary is unchanged.
RUN_SCRIPT_ADDENDUM += "Pass \"timeout_ms\": 120000 on every run_script call.\n"


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
    daemon_env: tuple[tuple[str, str], ...] = ()  # Amendment 6: extra daemon + MCP env (the script flag)
    prompt_addendum: str | None = None  # Amendment 6: text appended to the system prompt

    @property
    def binary_pin(self) -> str:
        """Key in pins.json ``cua_main`` of this app's binary hash. The script app is the same build re-signed with
        its own identifier, so its file hash differs from the main app's (Amendment 6)."""
        return "script_binary_sha256" if self.app == CUA_SCRIPT_APP else "binary_sha256"

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
    "cc-cua-driver-script": CuaBuild(
        "cc-cua-driver-script",
        "main",
        CUA_SCRIPT_APP,
        SCRIPT_SOCKET,
        CUA_SCRIPT_DIR / "daemon-state",
        CUA_MAIN_DIR / "skills" / "cua-driver",
        daemon_env=(SCRIPT_FLAG,),
        prompt_addendum=RUN_SCRIPT_ADDENDUM,
    ),
}
CUA_ARMS = tuple(CUA_BUILDS)

# Amendment 9 (CUA-1241): arc-driver, the MCP server of the third-party package arc-cua (github.com/shhivv/arc-cua,
# MIT), pinned to release 0.1.1 and installed in the VM only (tools/arc_driver/install_arc_driver.sh). It is a Python
# stdio server with no daemon. It is started through ArcDriverBench.app, a small ad-hoc signed launcher with its own
# bundle id, so that the launcher, not Terminal, is the process TCC holds responsible (Accessibility and Screen
# Recording are granted to that bundle id only).
ARC_DIR = WORK / "arc-cua"
ARC_APP = ARC_DIR / "ArcDriverBench.app"
ARC_LAUNCHER = ARC_APP / "Contents/MacOS/arc-launch"
ARC_BUNDLE_ID = "com.trycua.bench.arcdriver"
ARC_VENV = ARC_DIR / "venv"
ARC_PYTHON = ARC_VENV / "bin/python"
ARC_HOME = ARC_DIR / "home"
ARC_SERVER = "arc"
# -I: no user site, no PYTHON* variables, no script or working directory on sys.path; -B: no .pyc files, so the
# installed tree keeps the hash pinned in pins.json.
ARC_ARGS = ("-I", "-B", "-m", "arc_cua", "mcp")
# The Chromium switch arc-driver asks for (its FORCE_ACCESSIBILITY constant, driver.py:55 at 0.1.1). The pack's
# Electron apps already append it themselves (app.commandLine.appendSwitch), so for them it changes nothing.
FORCE_ACCESSIBILITY_FLAG = "--force-renderer-accessibility"
ARC_STATUS_REQUIRED = ("accessibility", "screen_recording", "background_input")
ARC_TOOLS = (
    "status", "apps", "windows", "observe", "act", "settle", "wait", "commands", "run_command", "release",
    "screenshot", "click_at", "drag", "scroll_at", "press", "type_text",
)  # fmt: skip


@dataclass(frozen=True)
class ArcBuild:
    """The arc-driver arm: pinned package in its own venv, started through the launcher app."""

    arm: str
    launcher: Path
    python: Path
    home: Path
    force_accessibility: bool = True  # A9.3: Chrome and Electron start with FORCE_ACCESSIBILITY_FLAG in this arm

    @property
    def argv(self) -> list[str]:
        return [str(self.launcher), str(self.python), *ARC_ARGS]


ARC_BUILDS = {"cc-arc-driver": ArcBuild("cc-arc-driver", ARC_LAUNCHER, ARC_PYTHON, ARC_HOME)}
ARC_ARMS = tuple(ARC_BUILDS)


def arc_env(home: Path) -> dict[str, str]:
    """Environment of the arc-driver server: its own empty HOME, a closed PATH. Its MCP path reads no variables and
    makes no network calls (review in the scoping notes); the two variables below are belt and braces."""
    return {"HOME": str(home), "PATH": "/usr/bin:/bin", "PYTHONNOUSERSITE": "1", "DO_NOT_TRACK": "1"}


def force_accessibility(arm: str) -> bool:
    build = ARC_BUILDS.get(arm)
    return bool(build and build.force_accessibility)

# Amendment 11: "Claude Desktop 2.31226.0 computer-use helper via a minimal adapter". The helper binary of Claude
# Desktop 2.31226.0 (Contents/Helpers/app-cu-helper, Anthropic-signed, used unmodified in place inside the unpacked,
# never-launched app) does the input; tools/claude_cu_helper/cu_helper_mcp.py exposes it as MCP server
# "claude-cu-helper" and adds a window list (winlist, CGWindowList) and window screenshots (/usr/sbin/screencapture).
# VM only (tools/claude_cu_helper/install_cu_helper.sh). Cua Driver is not used by this arm.
CU_HELPER_DIR = WORK / "claude-cu-helper"
CU_HELPER_ADAPTER = CU_HELPER_DIR / "cu_helper_mcp.py"
CU_HELPER_WINLIST = CU_HELPER_DIR / "winlist"
CU_HELPER_LAUNCHER = CU_HELPER_DIR / "cu-disclaim"  # starts the helper as its own responsible process (A11)
CU_DESKTOP_DIR = WORK / "claude-desktop-2.31226.0"
CU_HELPER_BIN = CU_DESKTOP_DIR / "extracted" / "Claude.app" / "Contents" / "Helpers" / "app-cu-helper"
CU_HELPER_SERVER = "claude-cu-helper"
CU_HELPER_PYTHON = Path("/opt/homebrew/bin/python3")
CU_HELPER_LABEL = "Claude Desktop 2.31226.0 computer-use helper via a minimal adapter"


@dataclass(frozen=True)
class CuHelperBuild:
    arm: str
    adapter: Path
    helper: Path
    winlist: Path
    launcher: Path

    def argv(self, actions: list[str]) -> list[str]:
        return [str(CU_HELPER_PYTHON), "-I", "-B", str(self.adapter), "--helper", str(self.helper),
                "--winlist", str(self.winlist), "--launcher", str(self.launcher), "--actions", ",".join(actions)]


CU_HELPER_BUILDS = {
    "cc-claude-cu-helper": CuHelperBuild(
        "cc-claude-cu-helper", CU_HELPER_ADAPTER, CU_HELPER_BIN, CU_HELPER_WINLIST, CU_HELPER_LAUNCHER
    )
}
CU_HELPER_ARMS = tuple(CU_HELPER_BUILDS)


def cu_helper_env() -> dict[str, str]:
    """The adapter's environment: closed PATH, real HOME (the helper keeps no state there), nothing else."""
    return {"HOME": os.environ.get("HOME", "/tmp"), "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "DO_NOT_TRACK": "1"}


def cu_helper_actions(pins: dict[str, Any] | None = None) -> list[str]:
    """The input tools the adapter offers: exactly the pinned list from the VM probe (A11)."""
    pins = pins if pins is not None else load_pins()
    return list((pins.get("claude_cu_helper") or {}).get("actions") or [])


def cu_helper_observed(build: CuHelperBuild | None = None) -> dict[str, Any]:
    """What is installed for the helper arm right now, keyed like pins.json ``claude_cu_helper``."""
    build = build or CU_HELPER_BUILDS["cc-claude-cu-helper"]
    zip_path = CU_DESKTOP_DIR / "Claude-2.31226.0.zip"
    return {
        "desktop_zip_sha256": sha256_file(zip_path) if zip_path.is_file() else None,
        "helper_sha256": sha256_file(build.helper) if build.helper.is_file() else None,
        "adapter_sha256": sha256_file(build.adapter) if build.adapter.is_file() else None,
        "winlist_sha256": sha256_file(build.winlist) if build.winlist.is_file() else None,
        "launcher_sha256": sha256_file(build.launcher) if build.launcher.is_file() else None,
    }


def check_cu_helper_pins(pins: dict[str, Any], observed: dict[str, Any]) -> list[tuple[str, str, str]]:
    want = pins.get("claude_cu_helper", {})
    out = []
    for key in ("desktop_zip_sha256", "helper_sha256", "adapter_sha256", "winlist_sha256", "launcher_sha256"):
        ok = bool(want.get(key)) and observed.get(key) == want.get(key)
        out.append((f"pin claude_cu_helper.{key}", "pass" if ok else "fail", str(observed.get(key))
                    if ok else f"observed {observed.get(key)!r}, pinned {want.get(key)!r}"))
    return out


CODEX_CU_MCP_CANDIDATES = (
    WORK / "codex-access" / "mcp.json",
    WORK / "codex-access" / "mcp_cua_repl_computer.json",
)
# Amendment 10: arm B on the Codex app 26.1007.21159 with the browser surface on. Its MCP config is written by
# tools/make_codex_cu_mcp.py --surfaces browser,computer from that app's plugin entry.
CODEX_ARMS = {
    "cc-codex-cu": WORK / "codex-access" / "mcp.json",
    "cc-codex-cu-1007-browser": WORK / "codex-access" / "mcp-1007-browser.json",
}
CWD_ROOT = Path("/tmp/cdb-bench-cwd/work")  # outside every git checkout, same path for both arms

ARM_DESCRIPTIONS = {
    "cc-cua-driver": "Claude Code + Cua Driver 0.34.0 MCP + Cua Driver skill (0.34.0 release)",
    "cc-cua-driver-main": "Claude Code + Cua Driver built from main (pinned commit) MCP + the skill of that commit",
    "cc-cua-driver-main-skill": "The same as cc-cua-driver-main, with the skill's SKILL.md appended to the system prompt",
    "cc-cua-driver-script": "The main build in its own app with CUA_DRIVER_EXPERIMENTAL_SCRIPT=1 (run_script on) and the run_script addendum in the system prompt",
    "cc-codex-cu": "Claude Code + Codex computer-use cua_repl MCP (server codex-cu), no skill",
    "cc-codex-cu-1007-browser": "Claude Code + Codex 26.1007.21159 cua_repl MCP (server codex-cu), surfaces browser+computer, no skill (Amendment 10)",
    "cc-claude-cu-helper": "Claude Code + " + CU_HELPER_LABEL + " (MCP server claude-cu-helper; Amendment 11), no skill",
    "cc-arc-driver": "Claude Code + arc-driver (arc-cua 0.1.1) MCP (server arc) through its own launcher app, no skill; "
    "Chrome and Electron started with --force-renderer-accessibility",
}

SKILL_PROMPT_HEADER = (
    "\n\nThe instructions of the Cua Driver skill follow. They apply to the `cua` MCP server. The skill's other "
    "files, which these instructions refer to, are in .claude/skills/cua-driver/ and can be read with the Read tool."
    "\n\n"
)


def system_prompt_for(arm: str) -> str:
    """The shared SYSTEM_PROMPT; for a skill-in-prompt arm, followed by its build's SKILL.md, verbatim."""
    build = CUA_BUILDS.get(arm)
    if build is None:
        return SYSTEM_PROMPT
    prompt = SYSTEM_PROMPT
    if build.skill_in_prompt:
        prompt += SKILL_PROMPT_HEADER + (build.skills / "SKILL.md").read_text("utf-8").strip() + "\n"
    if build.prompt_addendum:
        prompt += build.prompt_addendum
    return prompt


def build_env(arm: str, home: Path) -> dict[str, str]:
    """cua_env plus the build's own daemon variables (Amendment 6)."""
    env = cua_env(home)
    build = CUA_BUILDS.get(arm)
    if build is not None:
        env.update(dict(build.daemon_env))
    return env


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


def sha256_tree_nocache(path: Path) -> str:
    """sha256_tree without __pycache__ folders (the arc venv: the server runs with -B, this is a second guard)."""
    digest = hashlib.sha256()
    for item in sorted(p for p in path.rglob("*") if p.is_file() and "__pycache__" not in p.parts):
        digest.update(str(item.relative_to(path)).encode())
        digest.update(sha256_file(item).encode())
    return digest.hexdigest()


def arc_site_packages(venv: Path = ARC_VENV) -> Path | None:
    found = sorted(venv.glob("lib/python3.*/site-packages"))
    return found[0] if found else None


def arc_dist_version(site: Path | None) -> str | None:
    """The installed arc-cua version from its dist-info METADATA (what the status tool reports)."""
    for meta in sorted((site or Path("/nonexistent")).glob("arc_cua-*.dist-info/METADATA")):
        for line in meta.read_text("utf-8", "replace").splitlines():
            if line.startswith("Version:"):
                return line.split(":", 1)[1].strip()
    return None


def arc_observed(build: ArcBuild | None = None) -> dict[str, Any]:
    """What is installed for the arc arm right now, keyed like pins.json ``arc_driver``."""
    build = build or ARC_BUILDS["cc-arc-driver"]
    site = arc_site_packages(build.python.parent.parent)
    return {
        "version": arc_dist_version(site),
        "package_tree_sha256": sha256_tree_nocache(site / "arc_cua") if site and (site / "arc_cua").is_dir() else None,
        "site_packages_tree_sha256": sha256_tree_nocache(site) if site else None,
        "launcher_sha256": sha256_file(build.launcher) if build.launcher.is_file() else None,
        "python_version": _run([str(build.python), "-I", "-c", "import sys; print(sys.version.split()[0])"]).stdout.strip()
        if build.python.exists()
        else None,
    }


def check_arc_pins(pins: dict[str, Any], observed: dict[str, Any]) -> list[tuple[str, str, str]]:
    """Every pinned arc value must equal the observed one; a missing pin or a missing install fails."""
    want = pins.get("arc_driver", {})
    out = []
    for key in ("version", "package_tree_sha256", "site_packages_tree_sha256", "launcher_sha256", "python_version"):
        ok = bool(want.get(key)) and observed.get(key) == want.get(key)
        out.append((f"pin arc_driver.{key}", "pass" if ok else "fail", str(observed.get(key))
                    if ok else f"observed {observed.get(key)!r}, pinned {want.get(key)!r}"))
    return out


def arc_status_problems(status: dict[str, Any]) -> list[str]:
    """Preflight rule of A9.4: the status tool must report accessibility, screen recording and background input."""
    perms = status.get("permissions") or {}
    flat = {**perms, "background_input": status.get("background_input")}
    return [key for key in ARC_STATUS_REQUIRED if flat.get(key) is not True]


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
                    "env": build_env(arm, build.home),
                }
            }
        }
        path = run_dir / f"mcp-{arm}.json"
        path.write_text(json.dumps(config, indent=2) + "\n", "utf-8")
        return path, "cua"
    if arm in ARC_BUILDS:
        build = ARC_BUILDS[arm]
        command, *args = build.argv  # absolute paths: nothing is resolved through PATH or fetched at trial time
        config = {
            "mcpServers": {
                ARC_SERVER: {"type": "stdio", "command": command, "args": args, "env": arc_env(build.home)}
            }
        }
        path = run_dir / f"mcp-{arm}.json"
        path.write_text(json.dumps(config, indent=2) + "\n", "utf-8")
        return path, ARC_SERVER
    if arm in CU_HELPER_BUILDS:
        build = CU_HELPER_BUILDS[arm]
        command, *args = build.argv(cu_helper_actions())
        config = {"mcpServers": {CU_HELPER_SERVER: {"type": "stdio", "command": command, "args": args,
                                                     "env": cu_helper_env()}}}
        path = run_dir / f"mcp-{arm}.json"
        path.write_text(json.dumps(config, indent=2) + "\n", "utf-8")
        return path, CU_HELPER_SERVER
    if arm in CODEX_ARMS:
        source = (
            resolve_codex_cu_config(codex_cu_config)
            if arm == "cc-codex-cu"
            else (CODEX_ARMS[arm] if CODEX_ARMS[arm].is_file() else None)
        )
        if source is None:
            raise FileNotFoundError(
                f"Codex computer-use MCP config not found for arm {arm}; the arm is disabled"
            )
        data = json.loads(source.read_text("utf-8"))
        servers = list((data.get("mcpServers") or {}))
        if len(servers) != 1:
            raise ValueError(f"{source}: expected exactly one MCP server, found {servers}")
        path = run_dir / f"mcp-{arm}.json"
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
    env_extra: dict[str, str] | None = None,
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
        env={**cua_env(home), **(env_extra or {})},
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
