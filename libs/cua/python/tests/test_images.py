"""Canonical images through the native binding (pure helpers: no registry)."""

from __future__ import annotations

import cua
import pytest
from cua.images import Image


def test_canonical_images(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in ("CUA_IMAGE_LINUX", "CUA_DEFAULT_LINUX_IMAGE", "CUA_SANDBOX_LINUX_CONTAINER_IMAGE"):
        monkeypatch.delenv(name, raising=False)
    assert Image.linux() == "ghcr.io/trycua/linux:24.04"
    assert Image.windows() == "ghcr.io/trycua/windows:2022"
    assert Image.macos() == "ghcr.io/trycua/macos:26"
    assert Image.macos("sequoia") == "ghcr.io/trycua/macos:15"
    # from_registry is literal; aliases are CLI words only.
    assert Image.from_registry("ubuntu:24.04") == "ubuntu:24.04"
    assert cua.image_alias("ubuntu:24.04") is None
    assert cua.image_alias("ubuntu") == "ghcr.io/trycua/linux:24.04"
    assert Image.from_registry("python:3.12-slim") == "python:3.12-slim"
    assert cua.normalize_image("python:3.12-slim") == "docker.io/library/python:3.12-slim"
    monkeypatch.setenv("CUA_IMAGE_LINUX", "localhost:5000/trycua/linux:24.04")
    assert Image.linux() == "localhost:5000/trycua/linux:24.04"


def test_bad_backend_is_invalid_argument() -> None:
    with pytest.raises(cua.CuaError.InvalidArgument):
        cua.resolve_image("x", "kvm", None)


def _published(ref: str) -> bool:
    import json
    from pathlib import Path

    catalog = json.loads(
        (Path(__file__).resolve().parents[3] / "images/sandbox-images.json").read_text()
    )
    return next((i["published"] for i in catalog["images"] if i["ref"] == ref), True)


@pytest.mark.parametrize(
    ("make", "ref"),
    [
        (lambda: Image.linux(tier="slim"), "ghcr.io/trycua/linux:24.04-slim"),
        (lambda: Image.macos(tier="xcode"), "ghcr.io/trycua/macos:26-xcode"),
        (lambda: Image.omarchy(), "ghcr.io/trycua/omarchy:edge"),
    ],
)
def test_tiers_and_omarchy(make, ref: str) -> None:
    if _published(ref):
        assert make() == ref
    else:
        with pytest.raises(cua.CuaError.ImageNotPublished, match="not published yet"):
            make()


def test_tier_defaults_and_errors(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("CUA_IMAGE_LINUX", raising=False)
    assert Image.linux(tier="full") == "ghcr.io/trycua/linux:24.04"
    with pytest.raises(cua.CuaError.InvalidArgument):
        Image.linux(tier="xcode")
    with pytest.raises(cua.CuaError.InvalidArgument):
        Image.macos(tier="tiny")
