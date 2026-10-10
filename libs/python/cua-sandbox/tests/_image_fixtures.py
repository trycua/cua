"""Offline image manifests for the Fleet runtime/image rule.

The cua SDK's ``fleet_resolve_runtime`` reads the image's registry manifest.
Unit tests never reach a registry: the autouse fixture in ``conftest.py``
routes the call through the pure ``fleet_check_runtime`` with the variants
below (what those manifests would say). Unlisted images are unreadable, so
the runtime falls back to the reference, exactly as for an offline registry.
Live suites (``tests/live``) keep the real, registry-reading resolver.
"""

from __future__ import annotations

from typing import Any, Callable, Optional

DESKTOP = "ghcr.io/trycua/cua-desktop-linux"

VARIANTS = {
    "registry.example/workspace:latest": "container-disk",
    "registry.example/workspace@sha256:0123": "container-disk",
    "registry.example/workspace@sha256:abc": "container-disk",
    "example:latest": "container-disk",
    "123456789012.dkr.ecr.us-west-2.amazonaws.com/example-workspace:main-bac7daa3": "container-disk",
    "ghcr.io/trycua/minecraft-workspace:latest": "container-disk",
    f"{DESKTOP}:latest": "container-disk",
    f"{DESKTOP}:docker-latest": "rootfs",
}

#: The binding's real ``fleet_resolve_runtime`` (set by the fixture).
REAL: Optional[Callable[..., Any]] = None


def offline_resolver(native: Any) -> Callable[[Optional[str], str], str]:
    def resolve(runtime: Optional[str], image: str) -> str:
        return native.fleet_check_runtime(runtime, image, VARIANTS.get(image))

    return resolve
