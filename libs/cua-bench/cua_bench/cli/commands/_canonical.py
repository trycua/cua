"""The canonical images tasks run on (``ghcr.io/trycua/<os>``).

What ``cb image list|info`` and ``cb platform list|info`` show since cua-bench
0.3, replacing the retired local golden-image store. Each canonical image is
one registry index carrying its variants (a rootfs for containers, a
``-disk`` containerDisk for VMs, a Lume image for macOS); ``--kind``
picks one.
"""

from __future__ import annotations

from typing import Any, Optional

from cua_bench.images import CANONICAL

#: Used when the SDK cannot be loaded (generated from cua_image::canonical).
_FALLBACK = CANONICAL

_ROWS = (
    ("linux", ("container", "vm"), ("local", "cloud"), "rootfs (container), -disk (VM)"),
    ("windows", ("vm",), ("local", "cloud"), "-disk containerDisk (VM-only)"),
    ("macos", ("vm",), ("local",), "Lume image (VM-only, Apple Silicon)"),
)

#: Old `cb image` / `cb platform` names and the canonical OS they map to.
LEGACY_PLATFORMS = {
    "linux-docker": "linux",
    "linux-qemu": "linux",
    "windows-qemu": "windows",
    "windows-docker": "windows",
    "macos-lume": "macos",
    "macos": "macos",
    "linux": "linux",
    "windows": "windows",
}

MIGRATION = (
    "cua-bench 0.3 runs tasks on registry images: the canonical ghcr.io/trycua/<os> images "
    "by default, or any image with --image <registry ref> (or setup_config.image). Build "
    "and push custom images with `cua image ...`; the local golden-image store is gone."
)


def canonical_ref(os_type: str) -> str:
    try:
        from cua_sandbox.image import canonical_image

        return canonical_image(os_type)
    except Exception:  # noqa: BLE001 - SDK not loadable: the known defaults
        return _FALLBACK[os_type]


def canonical_images() -> list[dict[str, Any]]:
    return [
        {
            "name": os_type,
            "os_type": os_type,
            "image": canonical_ref(os_type),
            "kinds": list(kinds),
            "on": list(where),
            "variants": variants,
        }
        for os_type, kinds, where, variants in _ROWS
    ]


def find(name: str) -> Optional[dict[str, Any]]:
    os_type = LEGACY_PLATFORMS.get(name.strip().lower(), name.strip().lower())
    for row in canonical_images():
        if row["name"] == os_type:
            return row
    return None
