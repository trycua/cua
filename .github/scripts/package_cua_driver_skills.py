#!/usr/bin/env python3
"""Build a versioned, integrity-checked CUA Driver skill archive."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import tempfile


MANIFEST_NAME = "skill-pack.json"
SCHEMA_VERSION = 1


def normalized_tar_info(info: tarfile.TarInfo) -> tarfile.TarInfo:
    """Remove runner identity and timestamp metadata from release entries."""
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    return info


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def skill_version(source: Path) -> str:
    for line in (source / "SKILL.md").read_text(encoding="utf-8").splitlines():
        if line.startswith("version:"):
            return line.partition(":")[2].strip()
    raise ValueError("SKILL.md frontmatter has no version")


def payload_files(source: Path) -> list[Path]:
    files: list[Path] = []
    for path in sorted(source.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"skill source contains a symlink: {path.relative_to(source)}")
        if path.is_file() and path.name != MANIFEST_NAME:
            files.append(path)
    if not any(path.relative_to(source).as_posix() == "SKILL.md" for path in files):
        raise ValueError("skill source does not contain SKILL.md")
    return files


def build_archive(
    source: Path,
    archive: Path,
    root_name: str,
    version: str,
    source_kind: str,
    git_commit: str | None,
) -> dict[str, object]:
    source = source.resolve()
    if not source.is_dir():
        raise ValueError(f"skill source is not a directory: {source}")
    if skill_version(source) != version:
        raise ValueError(
            f"SKILL.md version {skill_version(source)!r} does not match release {version!r}"
        )
    if git_commit is not None and (
        len(git_commit) != 40
        or any(character not in "0123456789abcdefABCDEF" for character in git_commit)
    ):
        raise ValueError("git commit must be a full 40-character hexadecimal SHA")

    files = payload_files(source)
    manifest: dict[str, object] = {
        "schema_version": SCHEMA_VERSION,
        "skill_version": version,
        "compatible_driver_version": version,
        "source": {
            "kind": source_kind,
            **({"git_commit": git_commit} if git_commit else {}),
        },
        "files": [
            {
                "path": path.relative_to(source).as_posix(),
                "sha256": sha256(path),
            }
            for path in files
        ],
    }

    archive.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cua-driver-skills-") as temporary:
        stage = Path(temporary) / root_name
        stage.mkdir()
        for path in files:
            destination = stage / path.relative_to(source)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, destination)
        (stage / MANIFEST_NAME).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        with tarfile.open(archive, "w:gz") as bundle:
            bundle.add(stage, arcname=root_name, filter=normalized_tar_info)
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--root-name", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-kind", choices=("release", "main", "local"), required=True)
    parser.add_argument("--git-commit")
    arguments = parser.parse_args()
    build_archive(
        arguments.source,
        arguments.archive,
        arguments.root_name,
        arguments.version,
        arguments.source_kind,
        arguments.git_commit,
    )


if __name__ == "__main__":
    main()
