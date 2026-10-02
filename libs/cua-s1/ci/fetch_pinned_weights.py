"""Download the pinned public Cua-S1-4B weights and verify every file.

Reads ``weights.lock.json`` (next to this script by default), downloads each
listed file from Hugging Face at the exact pinned commit into
``<dest>/<artifact name>``, and verifies its size and SHA-256 before anything
loads it. No token is used or needed: every artifact is public.

    python libs/cua-s1/ci/fetch_pinned_weights.py --dest "$RUNNER_TEMP/s1-models"

Prints one JSON summary line. Exits nonzero on any missing, extra, or
mismatched file. ``--verify-only`` checks an existing directory without
network access.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import time
from pathlib import Path
from typing import Any

LOCK_SCHEMA = "cua-s1/weights-lock/v1"
_REVISION = re.compile(r"[0-9a-f]{40}")
_SHA256 = re.compile(r"[0-9a-f]{64}")
_CHUNK = 8 * 1024 * 1024


def load_lock(path: Path) -> list[dict[str, Any]]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema") != LOCK_SCHEMA:
        raise ValueError(f"{path}: unsupported schema {document.get('schema')!r}")
    artifacts = document["artifacts"]
    names = set()
    for artifact in artifacts:
        name = artifact["name"]
        if not re.fullmatch(r"[A-Za-z0-9._-]+", name) or name in names:
            raise ValueError(f"invalid or duplicate artifact name: {name!r}")
        names.add(name)
        if not _REVISION.fullmatch(artifact["revision"]):
            raise ValueError(f"{name}: revision must be a full commit SHA")
        if not artifact["files"]:
            raise ValueError(f"{name}: no files listed")
        for relative, expected in artifact["files"].items():
            parts = Path(relative).parts
            if Path(relative).is_absolute() or ".." in parts or not parts:
                raise ValueError(f"{name}: unsafe file path {relative!r}")
            if not _SHA256.fullmatch(expected["sha256"]) or int(expected["size"]) < 0:
                raise ValueError(f"{name}/{relative}: invalid size or sha256")
    return artifacts


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(_CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def verify_artifact(root: Path, artifact: dict[str, Any]) -> list[str]:
    """Return a list of problems; empty means every listed file matches."""
    problems = []
    expected_files = artifact["files"]
    for relative, expected in expected_files.items():
        path = root / relative
        if not path.is_file() or path.is_symlink():
            problems.append(f"{artifact['name']}/{relative}: missing")
            continue
        size = path.stat().st_size
        if size != int(expected["size"]):
            problems.append(f"{artifact['name']}/{relative}: size {size} != {expected['size']}")
            continue
        actual = sha256_file(path)
        if actual != expected["sha256"]:
            problems.append(
                f"{artifact['name']}/{relative}: sha256 {actual} != {expected['sha256']}"
            )
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if relative == ".cache" or relative.startswith(".cache/") or path.is_dir():
            continue
        if relative not in expected_files:
            problems.append(f"{artifact['name']}/{relative}: not in the lock")
    return problems


def download_artifact(root: Path, artifact: dict[str, Any]) -> None:
    from huggingface_hub import snapshot_download

    snapshot_download(
        repo_id=artifact["repo_id"],
        revision=artifact["revision"],
        local_dir=str(root),
        allow_patterns=sorted(artifact["files"]),
        token=False,
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lock", type=Path, default=Path(__file__).with_name("weights.lock.json"))
    parser.add_argument("--dest", type=Path, required=True)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args(argv)

    artifacts = load_lock(args.lock)
    args.dest.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    summary: dict[str, Any] = {"artifacts": []}
    problems: list[str] = []
    for artifact in artifacts:
        root = args.dest / artifact["name"]
        started = time.perf_counter()
        if not args.verify_only:
            download_artifact(root, artifact)
        downloaded = time.perf_counter()
        found = verify_artifact(root, artifact)
        problems.extend(found)
        summary["artifacts"].append(
            {
                "name": artifact["name"],
                "repo_id": artifact["repo_id"],
                "revision": artifact["revision"],
                "path": str(root),
                "files": len(artifact["files"]),
                "bytes": sum(int(item["size"]) for item in artifact["files"].values()),
                "download_s": round(downloaded - started, 1),
                "verify_s": round(time.perf_counter() - downloaded, 1),
                "verified": not found,
            }
        )
    summary["verified"] = not problems
    print(json.dumps(summary, separators=(",", ":")))
    for problem in problems:
        print(f"weights verification failed: {problem}", file=sys.stderr)
    return 0 if not problems else 1


if __name__ == "__main__":
    raise SystemExit(main())
