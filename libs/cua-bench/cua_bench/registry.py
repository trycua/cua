"""Versioned dataset registry: ``name@version`` pinned to a git commit.

``cb run dataset <name>[@<version>]`` and ``cb interact --dataset`` resolve a
dataset through a registry index (JSON), then fetch exactly the pinned
content with a shallow, sparse git checkout into a per-version cache. A plain
name resolves to the highest version. Fetched versions are immutable, so a
cached one is used offline.

Index sources, first match wins:

1. ``CUA_BENCH_REGISTRY``: a path or http(s) URL of a registry JSON;
2. the index bundled with cua-bench (``cua_bench/registry.json``).

Two entry shapes are accepted (both in one list):

* cua-bench: ``{"name", "version", "description", "git_url",
  "git_commit_id", "path", "image"}``: one directory of task directories.
  ``image`` (optional) is the desktop image for tasks that name none: a
  :mod:`cua_bench.images` constant (``BENCH_WEB``) or a registry ref;
* Harbor (``harbor-framework/harbor`` ``registry.json``): ``{"name",
  "version", "description", "tasks": [{"name", "git_url", "git_commit_id",
  "path"}]}``: each task pinned on its own. Harbor task directories
  (``task.toml``) need the Harbor adapter to run; they are fetched as-is.

``CUA_REGISTRY_HOME`` (0.2.x) still points at a local ``<dir>/datasets/<name>``
tree and bypasses the index.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

BUNDLED_INDEX = Path(__file__).resolve().parent / "registry.json"
GIT_TIMEOUT_S = 300


class RegistryError(RuntimeError):
    """A dataset that cannot be resolved or fetched."""


@dataclass(frozen=True)
class TaskRef:
    name: str
    git_url: str
    git_commit_id: str
    path: str


@dataclass(frozen=True)
class DatasetEntry:
    name: str
    version: str
    description: str = ""
    #: cua-bench shape: one directory of tasks.
    git_url: Optional[str] = None
    git_commit_id: Optional[str] = None
    path: Optional[str] = None
    #: Harbor shape: each task pinned on its own.
    tasks: tuple[TaskRef, ...] = field(default_factory=tuple)
    #: Desktop image for the dataset's tasks that name none (a
    #: ``cua_bench.images`` constant such as ``BENCH_WEB``, or a registry ref).
    image: Optional[str] = None

    @property
    def default_image(self) -> Optional[str]:
        """``image`` as a ref (constants resolve through :mod:`cua_bench.images`)."""
        if not self.image:
            return None
        from cua_bench import images

        if re.fullmatch(r"BENCH_[A-Z0-9_]+", self.image) and hasattr(images, self.image):
            return images.image(self.image)
        return self.image

    @property
    def ref(self) -> str:
        return f"{self.name}@{self.version}"

    @property
    def is_harbor(self) -> bool:
        return bool(self.tasks) and not self.path


def cache_root() -> Path:
    if os.environ.get("CUA_BENCH_REGISTRY_CACHE"):
        return Path(os.environ["CUA_BENCH_REGISTRY_CACHE"])
    # $CUA_HOME, else ~/.cua (the Rust core's rule).
    home = os.environ.get("CUA_HOME") or str(Path.home() / ".cua")
    return Path(home) / "cbregistry"


def parse_ref(ref: str) -> tuple[str, Optional[str]]:
    """``name@version`` -> (name, version); a plain name has no version."""
    name, sep, version = ref.strip().partition("@")
    if not name:
        raise RegistryError(f"invalid dataset reference {ref!r}")
    return name, (version or None) if sep else None


def _version_key(version: str) -> tuple:
    parts = re.split(r"[.\-+]", version)
    return tuple((0, int(p)) if p.isdigit() else (1, p) for p in parts)


def _read_index_text(source: str) -> str:
    if re.match(r"^https?://", source):
        with urllib.request.urlopen(source, timeout=60) as reply:  # noqa: S310 - user-set URL
            return reply.read().decode("utf-8")
    return Path(source).expanduser().read_text(encoding="utf-8")


def load_index(source: Optional[str] = None) -> list[DatasetEntry]:
    """The registry's datasets (``CUA_BENCH_REGISTRY`` or the bundled index)."""
    source = source or os.environ.get("CUA_BENCH_REGISTRY") or str(BUNDLED_INDEX)
    try:
        raw = json.loads(_read_index_text(source))
    except (OSError, ValueError) as error:
        raise RegistryError(f"cannot read the dataset registry {source}: {error}") from error
    items = raw.get("datasets", raw) if isinstance(raw, dict) else raw
    entries = []
    for item in items:
        tasks = tuple(
            TaskRef(
                name=str(t.get("name") or Path(t["path"]).name),
                git_url=str(t["git_url"]),
                git_commit_id=str(t["git_commit_id"]),
                path=str(t["path"]),
            )
            for t in item.get("tasks") or ()
        )
        entries.append(
            DatasetEntry(
                name=str(item["name"]),
                version=str(item.get("version") or "latest"),
                description=str(item.get("description") or ""),
                git_url=item.get("git_url"),
                git_commit_id=item.get("git_commit_id"),
                path=item.get("path"),
                tasks=tasks,
                image=item.get("image") or None,
            )
        )
    return entries


def find_entry(ref: str, entries: list[DatasetEntry]) -> Optional[DatasetEntry]:
    name, version = parse_ref(ref)
    matches = [e for e in entries if e.name == name]
    if version not in (None, "latest"):
        matches = [e for e in matches if e.version == version]
    if not matches:
        return None
    return max(matches, key=lambda e: _version_key(e.version))


def _git(args: list[str], cwd: Optional[Path] = None) -> None:
    try:
        out = subprocess.run(
            ["git", *args], cwd=cwd, capture_output=True, text=True, timeout=GIT_TIMEOUT_S
        )
    except FileNotFoundError as error:
        raise RegistryError("git is not installed") from error
    except subprocess.TimeoutExpired as error:
        raise RegistryError(f"git {' '.join(args[:2])} timed out") from error
    if out.returncode != 0:
        raise RegistryError(f"git {' '.join(args)} failed: {out.stderr.strip()[-500:]}")


def fetch_pinned(git_url: str, commit: str, paths: list[str], dest: Path) -> Path:
    """Shallow, sparse checkout of ``paths`` at ``commit`` into ``dest``."""
    if (dest / ".cb-complete").exists():
        return dest
    tmp = dest.with_name(dest.name + ".partial")
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(parents=True)
    try:
        _git(["init", "-q"], tmp)
        _git(["remote", "add", "origin", git_url], tmp)
        _git(["sparse-checkout", "set", "--no-cone", *sorted(set(paths))], tmp)
        _git(["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", commit], tmp)
        _git(["-c", "advice.detachedHead=false", "checkout", "-q", "FETCH_HEAD"], tmp)
        (tmp / ".cb-complete").write_text(f"{git_url} {commit}\n")
        shutil.rmtree(dest, ignore_errors=True)
        tmp.rename(dest)
    except BaseException:
        shutil.rmtree(tmp, ignore_errors=True)
        raise
    return dest


def _safe(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9._@-]+", "_", name)


def materialize(entry: DatasetEntry, root: Optional[Path] = None) -> Path:
    """The local directory of an entry's tasks (fetched once per version)."""
    base = (root or cache_root()) / _safe(entry.ref)
    if entry.path:
        if not (entry.git_url and entry.git_commit_id):
            raise RegistryError(f"{entry.ref} has a path but no git_url/git_commit_id")
        repo = fetch_pinned(entry.git_url, entry.git_commit_id, [entry.path], base / "repo")
        dataset = repo / entry.path
        if not dataset.is_dir():
            raise RegistryError(f"{entry.ref}: {entry.path} is not in {entry.git_url}")
        return dataset
    if not entry.tasks:
        raise RegistryError(f"{entry.ref} lists no tasks")
    groups: dict[tuple[str, str], list[TaskRef]] = {}
    for task in entry.tasks:
        groups.setdefault((task.git_url, task.git_commit_id), []).append(task)
    tasks_dir = base / "tasks"
    tasks_dir.mkdir(parents=True, exist_ok=True)
    for index, ((url, commit), tasks) in enumerate(sorted(groups.items())):
        repo = fetch_pinned(url, commit, [t.path for t in tasks], base / f"repo{index}")
        for task in tasks:
            link = tasks_dir / _safe(task.name)
            if not link.exists():
                link.symlink_to(repo / task.path, target_is_directory=True)
    return tasks_dir


#: The public registry browser (the same datasets, with task previews).
REGISTRY_URL = "https://cua.ai/cuabench/registry"


def resolve_entry(ref: str, *, index: Optional[str] = None) -> tuple[Path, Optional[DatasetEntry]]:
    """A dataset reference to its local directory and index entry.

    The entry is None for a ``CUA_REGISTRY_HOME`` dataset (no index).
    """
    legacy = os.environ.get("CUA_REGISTRY_HOME")
    if legacy:
        name, _ = parse_ref(ref)
        path = Path(legacy) / "datasets" / name
        if path.is_dir():
            return path, None
        raise RegistryError(f"{name} is not in CUA_REGISTRY_HOME ({legacy})")
    entries = load_index(index)
    entry = find_entry(ref, entries)
    if entry is None:
        known = ", ".join(sorted({e.ref for e in entries})) or "none"
        raise RegistryError(
            f"dataset {ref!r} is not in the registry (known: {known}). "
            f"List them with `cb dataset list`; browse {REGISTRY_URL}"
        )
    return materialize(entry), entry


def resolve(ref: str, *, index: Optional[str] = None) -> Path:
    """A dataset reference (``name`` or ``name@version``) to a local directory."""
    return resolve_entry(ref, index=index)[0]


def list_entries(index: Optional[str] = None) -> list[dict[str, Any]]:
    return [
        {
            "name": e.name,
            "version": e.version,
            "ref": e.ref,
            "description": e.description,
            "format": "harbor" if e.is_harbor else "cua-bench",
            "source": f"{e.git_url}@{(e.git_commit_id or '')[:12]}:{e.path}" if e.path else None,
            "tasks": len(e.tasks) if e.tasks else None,
            "image": e.image,
        }
        for e in load_index(index)
    ]
