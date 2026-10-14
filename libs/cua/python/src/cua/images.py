"""Canonical images and the one image resolver (native ``cua_image``).

``Image.linux()`` is ``ghcr.io/trycua/linux:24.04`` (``CUA_IMAGE_LINUX``
overrides it), ``Image.windows()`` ``ghcr.io/trycua/windows:2022`` and
``Image.macos()`` ``ghcr.io/trycua/macos:26``: the full tier, with dev
tooling. ``tier="slim"`` picks the minimal image CI runs (``24.04-slim``)
and, on macOS, ``tier="xcode"`` adds a pinned Xcode. ``Image.omarchy()`` is
``ghcr.io/trycua/omarchy:edge`` (an amd64 VM). Tiers and images CI has not
published yet raise ``CuaError.ImageNotPublished``; pass the reference to
``Image.from_registry`` to use one anyway. ``Image.resolve(ref,
backend)`` returns the digest-pinned variant a backend runs (rootfs for
containers, the ``-disk`` containerDisk for VMs, Lume on a Mac).

With ``cua[sandbox]`` installed, ``cua.Image`` is the richer
``cua_sandbox.Image``; these helpers are the SDK-only fallback.
"""

from __future__ import annotations

from typing import Optional

from ._native import (
    ResolvedImage,
    canonical_image,
    canonical_image_tier,
    image_alias,
    omarchy_image,
    resolve_image,
)


class Image:
    """Canonical image references (plain strings for ``sandboxes().create``)."""

    @staticmethod
    def linux(version: Optional[str] = None, tier: Optional[str] = None) -> str:
        """``slim`` or ``full`` (the default)."""
        return canonical_image_tier("linux", version, tier)

    @staticmethod
    def windows(version: Optional[str] = None, tier: Optional[str] = None) -> str:
        return canonical_image_tier("windows", version, tier)

    @staticmethod
    def macos(version: Optional[str] = None, tier: Optional[str] = None) -> str:
        """``slim``, ``full`` (the default), ``xcode`` or ``xcode-<X.Y>``."""
        return canonical_image_tier("macos", version, tier)

    @staticmethod
    def omarchy(channel: Optional[str] = None) -> str:
        """Omarchy (Arch Linux, Hyprland) with cua-spacesd: an amd64 VM
        (``ghcr.io/trycua/omarchy:edge``; emulated on arm64 hosts)."""
        return omarchy_image(channel)

    @staticmethod
    def from_registry(reference: str) -> str:
        # Literal: `ubuntu:24.04` is docker.io/library/ubuntu:24.04. Aliases
        # are for Image.linux()/windows()/macos() and the CLI's bare words.
        return reference

    @staticmethod
    def resolve(
        reference: str, backend: str = "local", arch: Optional[str] = None
    ) -> ResolvedImage:
        return resolve_image(reference, backend, arch)


__all__ = [
    "Image",
    "ResolvedImage",
    "canonical_image",
    "canonical_image_tier",
    "image_alias",
    "omarchy_image",
    "resolve_image",
]
