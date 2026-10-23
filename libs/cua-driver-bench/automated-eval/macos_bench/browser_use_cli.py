from __future__ import annotations

import shlex
from dataclasses import dataclass
from pathlib import Path

import arms

ARM = "cx-browser-use-cli"
TOOL_DIR = Path.home() / ".cache/cua-bench-h2h/browser-use-cli"
TOOL_BIN_DIR = TOOL_DIR / "bin"
EXECUTABLE = TOOL_BIN_DIR / "browser-use"


@dataclass(frozen=True)
class Invocation:
    argv: tuple[str, ...]
    python: str | None = None


def require_chatgpt() -> None:
    if arms.codex_auth() != "chatgpt":
        raise ValueError(f"{ARM} requires CDB_CODEX_AUTH=chatgpt")


def match(command: str, executable: Path = EXECUTABLE) -> Invocation | None:
    try:
        wrapper = shlex.split(command)
        if len(wrapper) == 3 and wrapper[:2] == ["/bin/zsh", "-lc"]:
            command = wrapper[2]
        if any(c in command for c in "\n\r\x00$`\\;|&<>(){}[]*?~#!^"):
            return None
        argv = shlex.split(command)
    except ValueError:
        return None
    if not argv or argv[0] not in ("browser-use", str(executable)):
        return None
    return Invocation((str(executable), *argv[1:]))


def preflight() -> list[tuple[str, str, str]]:
    return [(ARM, "fail", "Browser Use CLI operational preflight is not yet implemented")]
