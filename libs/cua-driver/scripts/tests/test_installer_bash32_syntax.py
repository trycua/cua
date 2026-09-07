"""Guard the installer scripts against the bash 3.2 parser on macOS.

macOS ships bash 3.2 as ``/bin/bash``. While scanning for the closing paren of
a command substitution, that parser tracks single quotes even inside a quoted
heredoc body, so a lone apostrophe in an embedded Python comment makes the
whole script fail to parse with ``unexpected EOF while looking for matching
')'``. Newer bash parses the same file cleanly, so ``bash -n`` on a Linux CI
runner does not catch it.

These tests therefore pair a syntax check against the system bash (bash 3.2 on
macOS) with a portable structural check that holds on any host: apostrophes in
a heredoc body must be balanced.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
SCRIPTS = sorted((REPO_ROOT / "libs/cua-driver/scripts").glob("*.sh"))

HEREDOC_START = re.compile(
    r"<<-?\s*(?:'([A-Za-z_][A-Za-z0-9_]*)'"
    r"|\"([A-Za-z_][A-Za-z0-9_]*)\""
    r"|([A-Za-z_][A-Za-z0-9_]*))"
)


def _heredoc_bodies(text: str) -> list[tuple[int, str, str]]:
    """Yield ``(line number, delimiter, body)`` for each heredoc in ``text``."""
    lines = text.splitlines()
    found: list[tuple[int, str, str]] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        match = None if line.lstrip().startswith("#") else HEREDOC_START.search(line)
        if match:
            delimiter = match.group(1) or match.group(2) or match.group(3)
            body: list[str] = []
            cursor = index + 1
            while cursor < len(lines) and lines[cursor].strip() != delimiter:
                body.append(lines[cursor])
                cursor += 1
            found.append((index + 1, delimiter, "\n".join(body)))
            index = cursor
        index += 1
    return found


@pytest.mark.parametrize("script", SCRIPTS, ids=lambda path: path.name)
def test_installer_scripts_parse_with_the_system_bash(script: Path) -> None:
    completed = subprocess.run(
        ["/bin/bash", "-n", str(script)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr


@pytest.mark.parametrize("script", SCRIPTS, ids=lambda path: path.name)
def test_heredoc_apostrophes_are_balanced_for_bash32(script: Path) -> None:
    unbalanced = [
        (line, delimiter)
        for line, delimiter, body in _heredoc_bodies(script.read_text(encoding="utf-8"))
        if body.count("'") % 2
    ]
    assert not unbalanced, (
        f"{script.name}: heredoc(s) with an unbalanced apostrophe "
        f"{unbalanced}; bash 3.2 cannot parse these inside a command "
        "substitution. Reword the text to avoid the apostrophe."
    )
