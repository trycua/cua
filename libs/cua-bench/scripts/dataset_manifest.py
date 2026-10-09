"""Reproducible, read-only dataset manifest for CUA Bench task directories.

Does not import or execute untrusted task main.py files.
"""
import argparse
import hashlib
import json
from pathlib import Path

VERSION = "cua-dataset-manifest/v1"
IGNORED = {"__pycache__", ".git", ".venv", "node_modules", ".pytest_cache"}


def scan_dataset(root: Path):
    root = root.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("dataset must be a directory")
    if (root / "main.py").is_file():
        tasks = [root]
    else:
        entries = sorted(root.iterdir())
        for entry in entries:
            if entry.is_symlink() and entry.is_dir():
                raise ValueError(f"symlinked dataset task directory: {entry.name}")
        tasks = [p for p in entries if p.is_dir() and (p / "main.py").is_file()]
    if not tasks:
        raise ValueError("no task directories containing main.py")
    output = []
    for task in tasks:
        files = []
        for path in sorted(task.rglob("*")):
            relative = path.relative_to(root)
            if any(part in IGNORED for part in relative.parts):
                continue
            if path.is_symlink():
                raise ValueError(f"symlink in dataset task: {relative}")
            if not path.is_file():
                continue
            data = path.read_bytes()
            files.append({"path": relative.as_posix(), "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)})
        if not files:
            raise ValueError(f"empty task: {task}")
        output.append({"task": task.relative_to(root).as_posix(), "files": files})
    return {"schema_version": VERSION, "tasks": output}


def verify_dataset(root: Path, manifest: dict):
    if not isinstance(manifest, dict) or manifest.get("schema_version") != VERSION:
        raise ValueError("unsupported dataset manifest")
    expected = manifest.get("tasks")
    if not isinstance(expected, list) or not expected:
        raise ValueError("manifest has no tasks")
    actual = scan_dataset(root)
    if actual != manifest:
        raise ValueError("dataset differs from manifest (added, removed, or modified task files)")
    return len(actual["tasks"])


def main():
    parser = argparse.ArgumentParser(description="Create or verify CUA dataset task manifest")
    parser.add_argument("dataset", type=Path)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--output", type=Path)
    group.add_argument("--verify", type=Path)
    args = parser.parse_args()
    if args.verify:
        count = verify_dataset(args.dataset, json.loads(args.verify.read_text()))
        print(f"Verified {count} tasks without executing task source")
    else:
        manifest = scan_dataset(args.dataset)
        args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        print(f"Recorded {len(manifest['tasks'])} tasks: {args.output}")


if __name__ == "__main__":
    main()
