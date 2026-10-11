"""The unified image path: one native resolver, canonical images, the P0 fixes.

Hermetic: ``native().resolve_image`` is replaced by a fake (conftest switches
real registry reads off with ``CUA_IMAGE_RESOLVE=0``), runtimes are
recorders, and the session tests use a file credential store in tmp.
"""

from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from types import SimpleNamespace

import pytest
from cua_sandbox import Image, Sandbox, _sdk
from cua_sandbox._config import FLEET_CREDENTIALS_MISSING, has_fleet_session
from cua_sandbox.image import resolve_image_kind
from cua_sandbox.transport.env import EnvTransport


def _resolved(variant: str, os: str = "linux", ref: str = "r") -> SimpleNamespace:
    return SimpleNamespace(
        variant=variant,
        os=os,
        pinned_ref=f"{ref}@sha256:" + "0" * 64,
        reference=ref,
        spacesd=None,
    )


@pytest.fixture
def resolver(monkeypatch):
    """Fake ``cua.resolve_image``: refs map to results or native errors."""
    n = _sdk.native()
    answers: dict[str, object] = {}
    calls: list[tuple] = []

    def resolve_image(reference, backend, arch):
        calls.append((reference, backend, arch))
        answer = answers[reference]
        if isinstance(answer, BaseException):
            raise answer
        return answer

    monkeypatch.setattr(n, "resolve_image", resolve_image)
    return SimpleNamespace(answers=answers, calls=calls, n=n)


def test_rootfs_runs_as_a_container(resolver):
    resolver.answers["python:3.12-slim"] = _resolved("rootfs")
    image = resolve_image_kind(Image.from_registry("python:3.12-slim"))
    assert (image.kind, image.os_type) == ("container", "linux")
    # The short ref goes to the native resolver as given (it means docker.io).
    assert resolver.calls == [("python:3.12-slim", "local", None)]


def test_short_refs_mean_docker_hub_not_ghcr():
    n = _sdk.native()
    assert n.normalize_image("python:3.12-slim") == "docker.io/library/python:3.12-slim"
    assert n.normalize_image("localhost:5000/repo:1") == "localhost:5000/repo:1"


def test_a_container_disk_runs_as_a_vm(resolver):
    """The old oras resolver called a KubeVirt containerDisk a container."""
    ref = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34"
    resolver.answers[ref] = _resolved("containerdisk")
    assert resolve_image_kind(Image.from_registry(ref)).kind == "vm"


def test_windows_and_lume_images_carry_their_os(resolver):
    resolver.answers["ghcr.io/trycua/windows:2022"] = _resolved("containerdisk", os="windows")
    resolver.answers["ghcr.io/trycua/macos:26"] = _resolved("lume", os="macos")
    win = resolve_image_kind(Image.from_registry("ghcr.io/trycua/windows:2022"))
    mac = resolve_image_kind(Image.from_registry("ghcr.io/trycua/macos:26"))
    assert (win.kind, win.os_type) == ("vm", "windows")
    assert (mac.kind, mac.os_type) == ("vm", "macos")


def test_canonical_linux_is_resolved_like_any_ref(resolver):
    resolver.answers["ghcr.io/trycua/linux:24.04"] = _resolved("rootfs")
    assert resolve_image_kind(Image.linux()).kind == "container"
    assert resolver.calls[0][0] == "ghcr.io/trycua/linux:24.04"
    # An explicit kind is not second-guessed.
    assert resolve_image_kind(Image.linux(kind="vm")).kind == "vm"
    assert len(resolver.calls) == 1


def test_a_local_only_tag_runs_as_a_container(resolver):
    """`cua-e2e-mcp-probe:1` exists only in the engine: no registry error."""
    resolver.answers["cua-e2e-mcp-probe:1"] = resolver.n.CuaError.NotFound("no such image")
    assert resolve_image_kind(Image.from_registry("cua-e2e-mcp-probe:1")).kind == "container"


def test_unrunnable_images_are_a_typed_error_locally(resolver):
    resolver.answers["ghcr.io/me/mac:1"] = resolver.n.CuaError.Unsupported(
        "ghcr.io/me/mac:1: not a runnable image"
    )
    with pytest.raises(ValueError, match="not a runnable image"):
        resolve_image_kind(Image.from_registry("ghcr.io/me/mac:1"))


def test_refused_credentials_say_how_to_log_in(resolver):
    resolver.answers["ghcr.io/me/private:1"] = resolver.n.CuaError.Unauthenticated("401")
    with pytest.raises(PermissionError, match="docker login"):
        resolve_image_kind(Image.from_registry("ghcr.io/me/private:1"))


def test_resolution_switched_off_falls_back_to_a_container():
    # conftest sets CUA_IMAGE_RESOLVE=0: the real native call fails at once.
    image = resolve_image_kind(Image.from_registry("ghcr.io/me/anything:1"))
    assert image.kind == "container"


# ── server_port reaches the auto-selected local runtime ────────────────────


class _Stop(Exception):
    pass


async def test_server_port_reaches_the_local_runtime(resolver, monkeypatch):
    resolver.answers["registry.example/mcp:1"] = _resolved("rootfs")
    seen = []

    class Recorder:
        def __init__(self, **kwargs):
            self.server_port = None

        async def start(self, image, name, **opts):
            seen.append((image.kind, self.server_port))
            raise _Stop

    monkeypatch.setattr("cua_sandbox.runtime.docker.DockerRuntime", Recorder)
    with pytest.raises(_Stop):
        await Sandbox.create(
            Image.from_registry("registry.example/mcp:1"), local=True, server_port=8765
        )
    assert seen == [("container", 8765)]


# ── EnvTransport.request_service sends the caller's headers ────────────────


async def test_request_service_passes_headers():
    sent = []

    class Service:
        async def request(self, method, path, body, timeout_ms, headers=None):
            sent.append((method, path, body, headers))
            return SimpleNamespace(status=200, headers=[], body=b"{}")

    handle = SimpleNamespace(service=lambda name: Service())
    t = EnvTransport(env_factory=lambda: None, native_sandbox=handle)
    r = await t.request_service(
        "mcp",
        method="POST",
        path="/mcp",
        json_body={"jsonrpc": "2.0"},
        headers={"accept": "application/json, text/event-stream"},
    )
    assert r.status_code == 200
    method, path, body, headers = sent[0]
    assert (method, path, json.loads(body)) == ("POST", "/mcp", {"jsonrpc": "2.0"})
    assert {(h.name, h.value) for h in headers} == {
        ("accept", "application/json, text/event-stream"),
        ("content-type", "application/json"),
    }


# ── Missing Fleet credentials and the `cua auth login` session ─────────────


async def test_no_credentials_names_every_way_out():
    with pytest.raises(Exception) as info:
        await Sandbox.create(Image.from_registry("python:3.12-slim"), local=False)
    assert FLEET_CREDENTIALS_MISSING in str(info.value)
    assert "cua auth login" in FLEET_CREDENTIALS_MISSING
    assert "local=True" in FLEET_CREDENTIALS_MISSING


def test_the_cua_auth_login_session_counts_as_fleet_access(monkeypatch, tmp_path):
    monkeypatch.setenv("CUA_HOME", str(tmp_path))
    monkeypatch.setenv("CUA_CREDENTIAL_STORE", "file")
    monkeypatch.delenv("CUA_FLEET_SESSION", raising=False)
    assert has_fleet_session() is False
    expires = (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat()
    (tmp_path / "credentials.json").write_text(
        json.dumps({"access_token": "tok", "expires_at": expires})
    )
    assert has_fleet_session() is True
    assert Sandbox._uses_fleet(None) is True
    monkeypatch.setenv("CUA_FLEET_SESSION", "0")
    assert has_fleet_session() is False
