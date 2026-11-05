"""Finder metadata in the workspace: colour tags, Spotlight comments and the locked flag.

Tags and comments are extended attributes holding binary property lists. They are read and written with
/usr/bin/xattr (macOS's Python has no xattr functions), so this works for any user that can read the file.
"""

from __future__ import annotations

import os
import plistlib
import stat
import subprocess
from pathlib import Path

TAGS = "com.apple.metadata:_kMDItemUserTags"
COMMENT = "com.apple.metadata:kMDItemFinderComment"
XATTR = "/usr/bin/xattr"
COLOURS = {"Gray": 1, "Green": 2, "Purple": 3, "Blue": 4, "Yellow": 5, "Red": 6, "Orange": 7}


def _read(path: Path, name: str) -> object | None:
    done = subprocess.run([XATTR, "-px", name, str(path)], capture_output=True, text=True)
    if done.returncode != 0:
        return None
    raw = bytes.fromhex("".join(done.stdout.split()))
    try:
        return plistlib.loads(raw)
    except Exception:  # noqa: BLE001 - not a plist: report the raw text
        return raw.decode("utf-8", "replace")


def _write(path: Path, name: str, value: object) -> None:
    data = plistlib.dumps(value, fmt=plistlib.FMT_BINARY)
    subprocess.run([XATTR, "-wx", name, data.hex(), str(path)], check=True, capture_output=True)


def tags(path: Path) -> set[str]:
    """Tag names on a file (colour numbers dropped)."""
    value = _read(path, TAGS)
    if not isinstance(value, list):
        return set()
    return {str(v).split("\n", 1)[0] for v in value}


def set_tags(path: Path, names: list[str]) -> None:
    if not names:
        subprocess.run([XATTR, "-d", TAGS, str(path)], capture_output=True)
        return
    _write(path, TAGS, [f"{n}\n{COLOURS[n]}" if n in COLOURS else n for n in names])


def comment(path: Path) -> str:
    value = _read(path, COMMENT)
    return value if isinstance(value, str) else ""


def set_comment(path: Path, text: str) -> None:
    _write(path, COMMENT, text)


def locked(path: Path) -> bool:
    return bool(os.lstat(path).st_flags & stat.UF_IMMUTABLE)


def unlock_tree(root: Path) -> None:
    """Clear the locked flag everywhere under root so the reset can delete it."""
    if root.exists():
        subprocess.run(["/usr/bin/chflags", "-R", "nouchg", str(root)], capture_output=True)
