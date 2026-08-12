"""Pull KubeVirt containerDisk images through the OCI registry API."""

from __future__ import annotations

import hashlib
import os
import shutil
import tarfile
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import oras.provider
from cua_sandbox.registry.cache import CACHE_ROOT

_CONTAINER_DISK_PATHS = {"disk/disk.img", "./disk/disk.img"}
_LOCK_POLL_INTERVAL_SECONDS = 0.1


def pull_container_disk(
    ref: str,
    *,
    cache_root: Path | None = None,
    registry_factory: Callable[..., Any] = oras.provider.Registry,
) -> Path:
    """Pull a KubeVirt containerDisk and cache its qcow2 disk locally."""
    root = cache_root or (CACHE_ROOT / "container-disks")
    destination = root / hashlib.sha256(ref.encode()).hexdigest() / "disk.qcow2"
    if destination.exists():
        return destination

    destination.parent.mkdir(parents=True, exist_ok=True)
    lock_path = destination.with_suffix(".lock")
    lock_fd = _acquire_cache_lock(lock_path, destination)
    if lock_fd is None:
        return destination
    try:
        if destination.exists():
            return destination

        registry = registry_factory(auth_backend="token")
        container = registry.get_container(ref)
        registry.auth.load_configs(container)
        manifest = registry.get_manifest(ref)

        temporary_fd, temporary_name = tempfile.mkstemp(
            dir=destination.parent, prefix="disk.", suffix=".tmp"
        )
        os.close(temporary_fd)
        temporary = Path(temporary_name)
        try:
            for layer in reversed(manifest.get("layers", [])):
                response = registry.get_blob(container, layer["digest"], stream=True)
                try:
                    with tarfile.open(fileobj=response.raw, mode="r|*") as archive:
                        for member in archive:
                            if member.name not in _CONTAINER_DISK_PATHS or not member.isfile():
                                continue
                            source = archive.extractfile(member)
                            if source is None:
                                continue
                            with temporary.open("wb") as output:
                                shutil.copyfileobj(source, output)
                            temporary.replace(destination)
                            return destination
                finally:
                    close = getattr(response, "close", None)
                    if close is not None:
                        close()
        finally:
            temporary.unlink(missing_ok=True)
    finally:
        os.close(lock_fd)
        lock_path.unlink(missing_ok=True)

    raise FileNotFoundError(f"OCI image {ref!r} does not contain /disk/disk.img")


def _acquire_cache_lock(lock_path: Path, destination: Path) -> int | None:
    while True:
        try:
            return os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_RDWR)
        except FileExistsError:
            if destination.exists():
                return None
            time.sleep(_LOCK_POLL_INTERVAL_SECONDS)
