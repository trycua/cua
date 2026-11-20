#!/usr/bin/env python3
"""Fail a pull request whose squash-merge message Release Please cannot parse.

GitHub builds the squash commit from the PR title plus every commit message.
Release Please parses that whole message with @conventional-commits/parser and
silently drops the commit when parsing fails, so the change never reaches the
changelog or the release PR (trycua/cua#7637 was dropped this way: a commit body
carried ``Selector:(null)`` inside a code span). Parse the same message here,
with the parser version Release Please pins, before it can merge.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

PARSER_VERSION = "0.4.1"
PARSER_INTEGRITY = (
    "sha512-H2ZmUVt6q+KBccXfMBhbBF14NlANeqHTXL4qCL6QGbMzrc4HDXyzWuxPxPNbz71f/5UkR5DrycP5VO9u7crahg=="
)

PARSE_JS = """
const { parser } = require('@conventional-commits/parser');
const message = require('fs').readFileSync(process.argv[1], 'utf8');
try { parser(message); } catch (error) { console.log(error.message); process.exit(1); }
"""


def squash_message(title: str, commit_messages: list[str]) -> str:
    """The message GitHub produces for a squash merge: title, then each commit as a bullet."""

    bullets = [f"* {message.strip()}" for message in commit_messages if message.strip()]
    return "\n\n".join([title.strip(), *bullets]) + "\n"


def commit_messages(base: str, head: str) -> list[str]:
    log = subprocess.run(
        ["git", "log", "--reverse", "--format=%B%x00", f"{base}..{head}"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [message for message in log.split("\0") if message.strip()]


def install_parser(prefix: Path) -> None:
    packed = subprocess.run(
        ["npm", "view", f"@conventional-commits/parser@{PARSER_VERSION}", "dist.integrity"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if packed != PARSER_INTEGRITY:
        raise SystemExit(f"@conventional-commits/parser@{PARSER_VERSION} integrity mismatch")
    subprocess.run(
        [
            "npm", "install", "--prefix", str(prefix), "--ignore-scripts",
            "--no-audit", "--no-fund", f"@conventional-commits/parser@{PARSER_VERSION}",
        ],
        check=True,
        capture_output=True,
    )


def parse_error(message: str, prefix: Path) -> str | None:
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as handle:
        handle.write(message)
    try:
        result = subprocess.run(
            ["node", "-e", PARSE_JS, handle.name],
            env={"NODE_PATH": str(prefix / "node_modules"), "PATH": os.environ.get("PATH", "")},
            capture_output=True,
            text=True,
        )
    finally:
        Path(handle.name).unlink()
    return None if result.returncode == 0 else result.stdout.strip() or result.stderr.strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--title", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    args = parser.parse_args()
    message = squash_message(args.title, commit_messages(args.base, args.head))
    with tempfile.TemporaryDirectory() as tmp:
        prefix = Path(tmp)
        install_parser(prefix)
        error = parse_error(message, prefix)
    if error is None:
        print("Release Please can parse this pull request's squash message.")
        return 0
    lines = message.splitlines()
    print(f"::error::Release Please cannot parse this pull request's squash message: {error}")
    print("It would drop the change from the changelog. Reword the commit line named above")
    print("(for example, move text shaped like `Name:(` out of the commit body), or, after merge,")
    print("add a BEGIN_COMMIT_OVERRIDE block to the PR body. Message start:")
    print(json.dumps(lines[:3]))
    return 1


if __name__ == "__main__":
    sys.exit(main())
