#!/usr/bin/env python3
"""Decide whether a pull request needs the `E2E: cua SDK` suite (about 77 minutes).

The workflow's `paths:` filter (e2e-cua-sdk.yml) decides whether the workflow
starts at all. Every entry in it is an input of the suite except one:
`docs/content/docs/**`. The suite reads the docs only through the fenced code
blocks of the pages (tests/e2e/cua-sdk/docs/extract.py: the `docs` lane runs
the blocks tagged test="..." and the coverage check joins them back to the
tags), so a page edit can change a result only if it changes a fence. This
script keeps the docs entry in the filter, so a PR that does change a tagged
block, or adds or removes one, still runs the suite, and drops it for
prose-only edits.

Output (stdout and $GITHUB_OUTPUT): `run=true|false` and `reason=...`.

It runs the suite (fails closed) when:
  * the event is not a pull request (nightly, manual dispatch);
  * the changed-file list is empty, or any file or git object cannot be read;
  * any changed path matches PATHS (the suite's non-docs inputs; a test pins
    this list to the workflow's `paths:`);
  * a changed docs page has a different fence list (language, info string, code)
    at the base and at the head, and either side has a test="..." fence;
  * a docs page is added, deleted, renamed or copied and either side has a
    test="..." fence.

Changed files outside both lists (the repo README, other apps) are ignored, as
the workflow's own `paths:` filter ignores them. Docs files that are not pages
(meta.json, redirects, images) are ignored: the suite does not read them.
"""

from __future__ import annotations

import importlib.util
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Optional, Sequence

REPO = Path(__file__).resolve().parents[2]
EXTRACT = REPO / "tests" / "e2e" / "cua-sdk" / "docs" / "extract.py"

DOCS_ROOT = "docs/content/docs/"
DOCS_GLOB = DOCS_ROOT + "**"
PAGE_SUFFIXES = (".md", ".mdx")

# Everything in e2e-cua-sdk.yml `on.pull_request.paths` except DOCS_GLOB.
PATHS = (
    "libs/cua/**",
    "libs/cua-spacesd/**",
    "libs/images/**",
    "libs/python/cua-sandbox/**",
    "tests/e2e/cua-sdk/**",
    # The browse-the-web scenario's MCP client session.
    "tests/e2e/browse/**",
    # The docs lane's tag policy.
    "docs/code-block-policy.json",
    # What the job runs around the suite: gVisor, the toolchain, the cache, the scope script.
    "scripts/images/install-gvisor.sh",
    ".github/actions/linux-rust-prep/**",
    ".github/actions/setup-protoc/**",
    ".github/actions/rust-cache/**",
    ".github/scripts/e2e_cua_sdk_scope.py",
    ".github/workflows/e2e-cua-sdk.yml",
)


def _glob(pattern: str) -> re.Pattern[str]:
    """A GitHub `paths:` glob: `**` crosses `/`, `*` and `?` do not."""
    out, i = [], 0
    while i < len(pattern):
        if pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("".join(out) + r"\Z")


_INPUTS = [_glob(p) for p in PATHS]


def is_input(path: str) -> bool:
    return any(rx.match(path) for rx in _INPUTS)


def is_page(path: str) -> bool:
    return path.startswith(DOCS_ROOT) and path.endswith(PAGE_SUFFIXES)


@dataclass(frozen=True)
class Change:
    status: str  # git name-status letter: A, M, D, R, C, T
    path: str  # the path at the head (at the base for D)
    old_path: Optional[str] = None  # the path at the base, for R and C


Reader = Callable[[str, str], Optional[str]]  # (side: "base" | "head", path) -> text or None


class Unreadable(Exception):
    pass


def _extract():
    spec = importlib.util.spec_from_file_location("e2e_docs_extract", EXTRACT)
    if spec is None or spec.loader is None:
        raise Unreadable(f"cannot load {EXTRACT}")
    module = importlib.util.module_from_spec(spec)
    # dataclasses resolves string annotations through sys.modules.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class _Text:
    """The one thing extract.fences asks of a path: read_text."""

    def __init__(self, text: str) -> None:
        self._text = text

    def read_text(self, encoding: str = "utf-8") -> str:
        return self._text


def _fences(extract, text: Optional[str], path: str):
    if text is None:
        raise Unreadable(f"{path} is not readable")
    return extract.fences(_Text(text), path)


def _tagged(fences) -> bool:
    return any("test" in f.attrs for f in fences)


def _signature(fences) -> list[tuple[str, str, bool]]:
    return [(f.info, f.code, f.generated) for f in fences]


def page_effect(change: Change, read: Reader, extract) -> Optional[str]:
    """Why this docs page change can change a suite result, or None."""
    base_path = change.old_path or change.path
    base = head = None
    if change.status != "A":
        base = _fences(extract, read("base", base_path), base_path)
    if change.status != "D":
        head = _fences(extract, read("head", change.path), change.path)
    sides = [f for f in (base, head) if f is not None]
    if not any(_tagged(f) for f in sides):
        return None  # no tagged block before or after: nothing the suite runs
    if change.status == "M":
        if _signature(base) != _signature(head):
            return f"{change.path} changes a code block on a page with tagged blocks"
        return None  # prose only
    return f"{change.path} ({change.status}) has tagged blocks"


def decide(
    changes: Sequence[Change],
    read: Reader,
    event: str = "pull_request",
    head_ref: str = "",
    extract=None,
) -> tuple[bool, str]:
    if event != "pull_request":
        return True, f"{event} runs the suite"
    if head_ref.startswith("release-please--"):
        return True, "release PR runs the suite"
    if not changes:
        return True, "no changed files listed, failing closed"
    pages: list[Change] = []
    for change in changes:
        touched = [change.path] + ([change.old_path] if change.old_path else [])
        for path in touched:
            if is_input(path):
                return True, f"{path} is an input of the suite"
        if any(is_page(p) for p in touched):
            pages.append(change)
    if pages:
        try:
            extract = extract or _extract()
            for change in pages:
                why = page_effect(change, read, extract)
                if why:
                    return True, why
        except Exception as exc:  # noqa: BLE001 - any doubt runs the suite
            return True, f"cannot compare docs blocks ({exc}), failing closed"
        return False, f"{len(pages)} docs page(s) changed prose only; no tagged block changed"
    return False, "no changed path is an input of the suite"


def _git(repo: Path, *args: str) -> bytes:
    done = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, check=False)
    if done.returncode != 0:
        raise Unreadable(f"git {' '.join(args)} failed: {done.stderr.decode(errors='replace').strip()}")
    return done.stdout


def parse_name_status(raw: bytes) -> list[Change]:
    """`git diff --name-status -z -M` output."""
    parts = raw.decode("utf-8").split("\0")
    if parts and parts[-1] == "":
        parts.pop()
    changes, i = [], 0
    while i < len(parts):
        status = parts[i][:1]
        if status in ("R", "C"):
            changes.append(Change(status, parts[i + 2], parts[i + 1]))
            i += 3
        else:
            changes.append(Change(status, parts[i + 1]))
            i += 2
    return changes


def git_changes(repo: Path, base: str, head: str) -> list[Change]:
    return parse_name_status(_git(repo, "diff", "--name-status", "-z", "-M", base, head))


def git_reader(repo: Path, base: str, head: str) -> Reader:
    def read(side: str, path: str) -> Optional[str]:
        try:
            return _git(repo, "show", f"{base if side == 'base' else head}:{path}").decode("utf-8")
        except (Unreadable, UnicodeDecodeError):
            return None

    return read


def main(argv: Sequence[str]) -> int:
    import argparse

    ap = argparse.ArgumentParser(description=(__doc__ or "").split("\n")[0])
    ap.add_argument("--repo", type=Path, default=Path.cwd())
    ap.add_argument("--base", default="HEAD^1", help="default: the merge commit's first parent")
    ap.add_argument("--head", default="HEAD")
    args = ap.parse_args(argv)
    event = os.environ.get("EVENT", "pull_request")
    head_ref = os.environ.get("HEAD_REF", "")
    try:
        if event != "pull_request":
            run, why = True, f"{event} runs the suite"
        else:
            run, why = decide(
                git_changes(args.repo, args.base, args.head),
                git_reader(args.repo, args.base, args.head),
                event,
                head_ref,
            )
    except Exception as exc:  # noqa: BLE001 - any doubt runs the suite
        run, why = True, f"cannot list the changed files ({exc}), failing closed"
    out = {"run": str(run).lower(), "reason": why}
    for key, value in out.items():
        print(f"{key}={value}")
    target = os.environ.get("GITHUB_OUTPUT")
    if target:
        with open(target, "a", encoding="utf-8") as fh:
            fh.writelines(f"{k}={v}\n" for k, v in out.items())
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
