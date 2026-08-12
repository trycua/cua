"""Pull KubeVirt containerDisk images through the OCI registry API."""

from __future__ import annotations

import hashlib
import logging
import os
import platform as _platform
import shutil
import tarfile
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any, Optional

import oras.provider
from cua_sandbox.registry.cache import CACHE_ROOT
from cua_sandbox.registry.media_types import VM_MEDIA_TYPES

logger = logging.getLogger(__name__)

_CONTAINER_DISK_PATHS = {"disk/disk.img", "./disk/disk.img"}
_LOCK_POLL_INTERVAL_SECONDS = 0.1

# ECR fronts the registry API with HTTP Basic auth, so oras' "token" backend fails with
# "This endpoint requires a token. Please use basic auth with a username or password."
# The basic backend reads the same ~/.docker/config.json entry and works for token-based
# registries too, because it only replays the credential the daemon config already holds.
_AUTH_BACKEND = "basic"

_INDEX_MEDIA_TYPES = frozenset(
    {
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
    }
)

_ARCHITECTURE_ALIASES = {
    "x86_64": "amd64",
    "x86-64": "amd64",
    "amd64": "amd64",
    "aarch64": "arm64",
    "arm64": "arm64",
}


def pull_container_disk(
    ref: str,
    *,
    cache_root: Path | None = None,
    architecture: Optional[str] = None,
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

        registry = registry_factory(auth_backend=_AUTH_BACKEND)
        container = registry.get_container(ref)
        registry.auth.load_configs(container)
        manifest = _resolve_platform_manifest(registry, ref, architecture or _host_architecture())

        temporary_fd, temporary_name = tempfile.mkstemp(
            dir=destination.parent, prefix="disk.", suffix=".tmp"
        )
        os.close(temporary_fd)
        temporary = Path(temporary_name)
        try:
            for layer in reversed(manifest.get("layers", [])):
                if _is_vm_layer(layer):
                    continue
                logger.info(
                    "Pulling containerDisk layer %s (%s bytes) from %s",
                    layer["digest"],
                    layer.get("size", "?"),
                    ref,
                )
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
                            logger.info("Cached containerDisk %s at %s", ref, destination)
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


def _host_architecture() -> str:
    machine = _platform.machine().lower()
    return _ARCHITECTURE_ALIASES.get(machine, machine)


def _repository(ref: str) -> str:
    """Strip the tag or digest from an image reference."""
    repository = ref.split("@", 1)[0]
    head, separator, tail = repository.rpartition(":")
    if separator and "/" not in tail:
        return head
    return repository


def _is_vm_layer(layer: dict) -> bool:
    """True for chunked VM-disk layers (lume/tart/qemu), which are never containerDisks."""
    media_type = layer.get("mediaType", "")
    return media_type in VM_MEDIA_TYPES or "part.number=" in media_type


def _resolve_platform_manifest(registry: Any, ref: str, architecture: str) -> dict:
    """Fetch the manifest for ``ref``, following image indexes to the platform child.

    Multi-arch images publish an OCI image index whose entries are per-platform child
    manifests, so it carries ``manifests`` and no ``layers`` of its own.
    """
    manifest = registry.get_manifest(ref)
    repository = _repository(ref)
    seen: set[str] = set()

    while manifest.get("mediaType") in _INDEX_MEDIA_TYPES or manifest.get("manifests"):
        entries = manifest.get("manifests", [])
        entry = _select_platform_entry(entries, architecture)
        if entry is None:
            available = ", ".join(
                sorted(
                    f"{(candidate.get('platform') or {}).get('os')}/"
                    f"{(candidate.get('platform') or {}).get('architecture')}"
                    for candidate in entries
                )
                or ["<none>"]
            )
            raise FileNotFoundError(
                f"OCI image {ref!r} has no linux/{architecture} manifest (available: {available})"
            )
        digest = entry["digest"]
        if digest in seen:
            raise FileNotFoundError(f"OCI image {ref!r} has a cyclic manifest index at {digest}")
        seen.add(digest)
        manifest = registry.get_manifest(f"{repository}@{digest}")

    return manifest


def _select_platform_entry(entries: list, architecture: str) -> Optional[dict]:
    for entry in entries:
        platform = entry.get("platform") or {}
        if platform.get("os") != "linux" or platform.get("architecture") != architecture:
            continue
        # Buildx publishes provenance/SBOM attestations as extra index entries.
        reference_type = (entry.get("annotations") or {}).get("vnd.docker.reference.type", "")
        if "attestation" in reference_type:
            continue
        return entry
    return None


def _acquire_cache_lock(lock_path: Path, destination: Path) -> int | None:
    while True:
        try:
            return os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_RDWR)
        except FileExistsError:
            if destination.exists():
                return None
            time.sleep(_LOCK_POLL_INTERVAL_SECONDS)
