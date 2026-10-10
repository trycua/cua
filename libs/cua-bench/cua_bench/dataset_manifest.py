"""Dataset identity and preflight verification for executable CUA Bench runs.

The manifest is read without importing untrusted task Python code.
"""
import hashlib
from pathlib import Path

VERSION = "cua-dataset-manifest/v1"
IGNORED = {"__pycache__", ".git", ".venv", "node_modules", ".pytest_cache"}


def scan_dataset(root: Path) -> dict:
    root = Path(root).resolve(strict=True)
    if not root.is_dir():
        raise ValueError("dataset must be a directory")
    if (root / "main.py").is_file():
        tasks = [root]
    else:
        entries = sorted(root.iterdir())
        for entry in entries:
            if entry.is_symlink() and entry.is_dir() and (entry / "main.py").is_file():
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
            digest = hashlib.sha256()
            size = 0
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
                    size += len(chunk)
            files.append({"path": relative.as_posix(), "sha256": digest.hexdigest(), "size": size})
        if not files:
            raise ValueError(f"empty task: {task}")
        output.append({"task": task.relative_to(root).as_posix(), "files": files})
    return {"schema_version": VERSION, "tasks": output}


def verify_dataset(root: Path, manifest: dict) -> int:
    if not isinstance(manifest, dict) or manifest.get("schema_version") != VERSION:
        raise ValueError("unsupported dataset manifest")
    expected = manifest.get("tasks")
    if not isinstance(expected, list) or not expected:
        raise ValueError("manifest has no tasks")
    actual = scan_dataset(root)
    if actual != manifest:
        raise ValueError("dataset differs from manifest (added, removed, or modified task files)")
    return len(actual["tasks"])
