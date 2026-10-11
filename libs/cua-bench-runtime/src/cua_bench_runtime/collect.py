"""Bounded copying from an untrusted, read-only guest filesystem."""

from __future__ import annotations

import hashlib
import os
import stat
from dataclasses import asdict, dataclass
from pathlib import Path

from cua_bench_runtime.errors import HarnessFailure, ValidationFailure
from cua_bench_runtime.guest_launch import CollectionBounds


@dataclass(frozen=True)
class CollectedFile:
    path: str
    sha256: str
    size: int


@dataclass(frozen=True)
class CollectionReport:
    files: tuple[CollectedFile, ...]
    skipped_symlinks: tuple[str, ...]
    total_bytes: int
    bounds: CollectionBounds

    def document(self) -> dict:
        return {
            "files": [asdict(item) for item in self.files],
            "skipped_symlinks": list(self.skipped_symlinks),
            "total_bytes": self.total_bytes,
            "bounds": asdict(self.bounds),
        }


def _safe_name(name: str) -> None:
    if not name or name in {".", ".."} or "/" in name or "\\" in name:
        raise ValidationFailure("collection encountered an unsafe path component")
    if len(os.fsencode(name)) > 255:
        raise ValidationFailure("collection encountered an overlong path component")


def collect_tree(
    source_root: Path,
    destination_root: Path,
    bounds: CollectionBounds,
    *,
    trusted_root: Path | None = None,
) -> CollectionReport:
    if source_root.is_symlink():
        raise HarnessFailure("collection source path contains a symlink")
    if trusted_root is not None:
        lexical_root = trusted_root.absolute()
        try:
            relative = source_root.absolute().relative_to(lexical_root)
        except ValueError as error:
            raise HarnessFailure("collection source escapes trusted root") from error
        trusted_root = trusted_root.resolve(strict=True)
        cursor = trusted_root
        for component in relative.parts:
            _safe_name(component)
            cursor = cursor / component
            try:
                status = os.lstat(cursor)
            except OSError as error:
                raise HarnessFailure("collection source path is unavailable") from error
            if stat.S_ISLNK(status.st_mode):
                raise HarnessFailure("collection source path contains a symlink")
        source_root = cursor.resolve(strict=True)
        if not source_root.is_relative_to(trusted_root):
            raise HarnessFailure("collection source escapes trusted root")
    else:
        source_root = source_root.resolve(strict=True)
    destination_root = destination_root.resolve()
    if not source_root.is_dir():
        raise HarnessFailure("collection source is not a real directory")
    if destination_root.exists():
        raise HarnessFailure("collection destination is not fresh")
    destination_root.mkdir(parents=True)
    files: list[CollectedFile] = []
    symlinks: list[str] = []
    total_bytes = 0

    def visit(source: Path, destination: Path, relative: Path, depth: int) -> None:
        nonlocal total_bytes
        if depth > bounds.max_depth:
            raise HarnessFailure("collection exceeded max_depth")
        try:
            entries = sorted(os.scandir(source), key=lambda item: item.name)
        except OSError as error:
            raise HarnessFailure("collection could not scan source") from error
        for entry in entries:
            _safe_name(entry.name)
            item_relative = relative / entry.name
            relative_text = item_relative.as_posix()
            try:
                status = entry.stat(follow_symlinks=False)
            except OSError as error:
                raise HarnessFailure("collection could not stat source entry") from error
            mode = status.st_mode
            if stat.S_ISLNK(mode):
                symlinks.append(relative_text)
                continue
            target = destination / entry.name
            if stat.S_ISDIR(mode):
                target.mkdir()
                visit(Path(entry.path), target, item_relative, depth + 1)
                continue
            if not stat.S_ISREG(mode):
                raise HarnessFailure(f"collection rejected special file: {relative_text}")
            if len(files) + 1 > bounds.max_files:
                raise HarnessFailure("collection exceeded max_files")
            if status.st_size > bounds.max_file_bytes:
                raise HarnessFailure("collection exceeded max_file_bytes")
            if total_bytes + status.st_size > bounds.max_total_bytes:
                raise HarnessFailure("collection exceeded max_total_bytes")
            flags = os.O_RDONLY
            if hasattr(os, "O_NOFOLLOW"):
                flags |= os.O_NOFOLLOW
            try:
                descriptor = os.open(entry.path, flags)
            except OSError as error:
                raise HarnessFailure("collection could not safely open source file") from error
            digest = hashlib.sha256()
            written = 0
            try:
                with os.fdopen(descriptor, "rb") as reader, target.open("xb") as writer:
                    while True:
                        chunk = reader.read(1024 * 1024)
                        if not chunk:
                            break
                        written += len(chunk)
                        if written > bounds.max_file_bytes:
                            raise HarnessFailure("collection exceeded max_file_bytes")
                        if total_bytes + written > bounds.max_total_bytes:
                            raise HarnessFailure("collection exceeded max_total_bytes")
                        digest.update(chunk)
                        writer.write(chunk)
                    writer.flush()
                    os.fsync(writer.fileno())
            except BaseException:
                if target.is_file() and not target.is_symlink():
                    target.unlink(missing_ok=True)
                raise
            if written != status.st_size:
                raise HarnessFailure("collection source changed while copying")
            total_bytes += written
            files.append(CollectedFile(relative_text, digest.hexdigest(), written))

    visit(source_root, destination_root, Path(), 0)
    return CollectionReport(
        files=tuple(files),
        skipped_symlinks=tuple(symlinks),
        total_bytes=total_bytes,
        bounds=bounds,
    )
