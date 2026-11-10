"""P2 sandbox parity: sidecars, private registry secrets and image layers
(hermetic). The native option objects are checked by capturing what
``cua_sandbox`` hands the SDK.
"""

from __future__ import annotations

import asyncio
import importlib

import pytest
from cua_sandbox import Container, Image, RegistrySecret, Sandbox
from cua_sandbox.containers import image_build, sidecars_of

sandbox_module = importlib.import_module("cua_sandbox.sandbox")

IMAGE = "registry.example/workspace@sha256:0123"


# ── pure ─────────────────────────────────────────────────────────────────


def test_registry_secret_never_shows_its_password():
    s = RegistrySecret("me", "hunter2")
    assert "hunter2" not in repr(s) and "<redacted>" in repr(s)
    assert s.username == "me"
    assert "hunter2" not in repr(Image.from_registry("ghcr.io/me/app:1", secret=s))
    with pytest.raises(ValueError):
        RegistrySecret("me", "")
    assert repr(RegistrySecret.from_env()) == (
        "RegistrySecret.from_env('CUA_REGISTRY_USERNAME', 'CUA_REGISTRY_PASSWORD')"
    )
    assert RegistrySecret.aws_ecr("us-west-2").username is None


def test_the_secret_follows_the_image_but_is_never_serialized():
    s = RegistrySecret("me", "hunter2")
    img = Image.from_registry("ghcr.io/me/app:1", secret=s).pip_install("mcp").env(A="1")
    assert img._secret is s, "chained builders keep the secret"
    assert "secret" not in str(img.to_dict()) and "hunter2" not in str(img.to_dict())
    # Equality ignores the credential (same image either way).
    assert Image.from_registry("x:1", secret=s) == Image.from_registry("x:1")


def test_sidecars_normalize_and_validate():
    [a, b] = sidecars_of(["redis:7-alpine", Container("postgres:16", ports=[5432])])
    assert a == Container("redis:7-alpine")
    assert b.ports == [5432]
    with pytest.raises(ValueError):
        Container("redis", ports=[0])
    with pytest.raises(ValueError):
        Container("")
    with pytest.raises(TypeError):
        sidecars_of([42])


def test_native_conversions():
    n = pytest.importorskip("cua")._native
    c = Container("redis:7-alpine", ports=[6379], env={"A": "1"}, command=["redis-server"]).native()
    assert (c.image, c.ports, c.env, c.command, c.name) == (
        "redis:7-alpine",
        [6379],
        {"A": "1"},
        ["redis-server"],
        None,
    )
    basic = RegistrySecret("me", "tok", registry="ghcr.io").native()
    assert isinstance(basic, n.RegistrySecret.BASIC) and basic.registry == "ghcr.io"
    assert isinstance(RegistrySecret.from_env().native(), n.RegistrySecret.FROM_ENV)
    assert isinstance(RegistrySecret.aws_ecr().native(), n.RegistrySecret.AWS_ECR)

    img = (
        Image.from_registry("python:3.12-slim")
        .apt_install("curl")
        .pip_install("mcp")
        .run("echo hi")
        .env(K="v")
        .copy("server.py", "/srv/server.py")
        .expose(8765)
    )
    build = image_build(img)
    assert [type(layer).__name__.rsplit(".", 1)[-1] for layer in build.layers] == [
        "APT_INSTALL",
        "PIP_INSTALL",
        "RUN",
    ]
    assert build.env == {"K": "v"} and build.ports == [8765]
    assert (build.files[0].source, build.files[0].destination) == ("server.py", "/srv/server.py")
    assert image_build(Image.from_registry("python:3.12-slim")) is None
    with pytest.raises(NotImplementedError, match="brew_install"):
        image_build(Image.from_registry("python:3.12-slim")._add_layer({"type": "brew_install"}))


def test_the_native_sdk_decides_warm_for_canonical_images():
    n = pytest.importorskip("cua")._native
    assert n.image_alias("linux") is not None
    assert n.resolve_image_with_secret is not None


# ── what cua_sandbox hands the SDK ───────────────────────────────────────


class _Captured(Exception):
    pass


def test_local_runc_opt_in_reaches_the_sdk(monkeypatch, tmp_path):
    """runtime="runc" is the explicit opt-in a local sidecar group needs."""
    pytest.importorskip("cua")
    from cua_sandbox.runtime import native as native_rt

    seen: list = []

    class _Sandboxes:
        async def create(self, options):
            seen.append(options)
            raise _Captured()

    class _Cua:
        def sandboxes(self):
            return _Sandboxes()

    monkeypatch.setattr(native_rt, "local_runtime", lambda: _Cua())
    monkeypatch.setattr(sandbox_module, "_TELEMETRY_AVAILABLE", False)
    img = Image.from_registry("python:3.12-slim", kind="container")
    with pytest.raises(_Captured):
        asyncio.run(Sandbox.create(img, local=True, runtime="runc", sidecars=["redis:7-alpine"]))
    assert seen[0].runtime == "runc"
    assert seen[0].kind == "container"
    assert [s.image for s in seen[0].sidecars] == ["redis:7-alpine"]
    seen.clear()
    with pytest.raises(_Captured):
        asyncio.run(Sandbox.create(img, local=True, sidecars=["redis:7-alpine"]))
    assert seen[0].runtime is None, "no silent choice: the SDK decides and refuses"
