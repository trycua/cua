#!/usr/bin/env python3
"""Create a deterministic CDB-S04 workspace from the pristine fixture."""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
FIXTURE = BUNDLE / "fixture"
def checked_workspace(value: Path) -> Path:
    workspace = value.resolve()
    bundle = BUNDLE.resolve()
    current = Path.cwd().resolve()
    forbidden = {Path(workspace.anchor), Path.home().resolve(), bundle, current}
    if (
        workspace in forbidden
        or bundle.is_relative_to(workspace)
        or current.is_relative_to(workspace)
        or len(workspace.parts) < 3
    ):
        raise ValueError(f"refusing unsafe workspace path: {workspace}")
    return workspace


def copy_tree(source: Path, target: Path) -> None:
    for item in sorted(source.rglob("*")):
        relative = item.relative_to(source)
        destination = target / relative
        if item.is_dir():
            destination.mkdir(parents=True, exist_ok=True)
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(item, destination)


def reset_workspace(workspace: Path) -> dict:
    workspace = checked_workspace(workspace)
    workspace.mkdir(parents=True, exist_ok=True)
    for target in workspace.iterdir():
        if target.is_symlink() or not target.is_dir():
            target.unlink()
        else:
            shutil.rmtree(target)
    copy_tree(FIXTURE, workspace)
    (workspace / ".runtime" / "browser").mkdir(parents=True, exist_ok=True)
    (workspace / ".runtime" / "cases").mkdir(parents=True, exist_ok=True)
    (workspace / ".runtime" / "wiki").mkdir(parents=True, exist_ok=True)
    facts = json.loads((workspace / "cdb-s04.workspace.json").read_text(encoding="utf-8"))
    return facts


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    args = parser.parse_args()
    facts = reset_workspace(args.workspace)
    sys.stdout.write(json.dumps({"ok": True, "workspace": str(args.workspace.resolve()), **facts}) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
