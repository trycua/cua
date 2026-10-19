"""Where, what kind, which engine: ``on``/``local``, ``kind`` and ``runtime``
on ``Sandbox.create``, hermetically.

Nothing starts: the native SDK's ``sandboxes().create`` is a recorder that
raises once it has the options. The user defaults come from the real native
settings (``cua.config_get``) over the temp ``CUA_HOME`` conftest gives every
test, so precedence is tested end to end: explicit > env >
``$CUA_HOME/config.toml`` > built-in.
"""

from __future__ import annotations

import importlib
import os
from types import SimpleNamespace
from typing import Any

import pytest
from cua_sandbox import (
    CloudOptions,
    Image,
    InvalidArgument,
    InvalidPlacement,
    Sandbox,
    _placement,
)
from cua_sandbox.runtime import native as native_rt
from cua_sandbox.runtime.docker import DockerRuntime

cua = pytest.importorskip("cua")
sandbox_module = importlib.import_module("cua_sandbox.sandbox")

ROOTFS = Image.from_registry("python:3.12-slim", kind="container")


class _Captured(Exception):
    pass


@pytest.fixture
def local_sdk(monkeypatch):
    """What NativeRuntime hands the SDK (``SandboxCreateOptions``)."""
    seen: list = []

    class _Sandboxes:
        async def create(self, options):
            seen.append(options)
            raise _Captured()

    monkeypatch.setattr(
        native_rt, "local_runtime", lambda: SimpleNamespace(sandboxes=lambda: _Sandboxes())
    )
    monkeypatch.setattr(sandbox_module, "_TELEMETRY_AVAILABLE", False)
    # These tests check what reaches the SDK, not the host: a Linux VM's QEMU
    # preflight must not depend on whether this machine has QEMU installed.
    monkeypatch.setattr("cua_sandbox.runtime.compat._has_qemu", lambda: True)
    return seen


@pytest.fixture
def notice(monkeypatch, caplog):
    monkeypatch.setattr(_placement, "_default_notice_shown", False)
    monkeypatch.delenv("CUA_QUIET_DEFAULT", raising=False)
    caplog.set_level("WARNING", logger="cua_sandbox.sandbox")

    def shown() -> list[str]:
        return [r.message for r in caplog.records if "defaulting to a local" in r.message]

    return shown


@pytest.fixture(autouse=True)
def _temp_config():
    """``cua.config_set`` here writes the temp CUA_HOME (conftest), never ~/.cua."""
    home = os.path.realpath(os.environ["CUA_HOME"])
    assert os.path.realpath(cua.config_path()).startswith(home)


# ── precedence: explicit > env > config > built-in ───────────────────────


async def test_builtin_default_is_local_with_the_notice(local_sdk, notice):
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, telemetry_enabled=False)
    assert local_sdk[0].on == "local"
    [message] = notice()
    assert "defaulting to a local sandbox" in message and "CUA_QUIET_DEFAULT=1" in message


async def test_env_beats_config(monkeypatch, local_sdk, notice):
    cua.config_set("default.on", "cloud")
    monkeypatch.setenv("CUA_DEFAULT_ON", "local")
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, telemetry_enabled=False)
    assert local_sdk
    assert not notice(), "the notice is only for the built-in default"


async def test_explicit_beats_env(monkeypatch, local_sdk):
    monkeypatch.setenv("CUA_DEFAULT_ON", "cloud")
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, local=True, telemetry_enabled=False)
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, on="local", telemetry_enabled=False)
    assert len(local_sdk) == 2


async def test_default_kind_and_runtime_come_from_config(local_sdk):
    cua.config_set("default.kind", "container")
    cua.config_set("default.runtime", "runc")
    with pytest.raises(_Captured):
        await Sandbox.create(Image.from_registry("python:3.12-slim"), telemetry_enabled=False)
    assert (local_sdk[0].kind, local_sdk[0].runtime) == ("container", "runc")


async def test_a_default_runtime_that_does_not_fit_is_skipped(monkeypatch, local_sdk):
    monkeypatch.setenv("CUA_DEFAULT_RUNTIME", "runc")
    with pytest.raises(_Captured):
        await Sandbox.create(Image.linux(), on="local", kind="vm", telemetry_enabled=False)
    [options] = local_sdk
    assert (options.kind, options.runtime) == ("vm", None), "runc does not run VMs"
    assert options.image.startswith("vm:")


# ── on / local ───────────────────────────────────────────────────────────


async def test_on_and_a_contradicting_local_is_invalid(local_sdk):
    with pytest.raises(InvalidArgument, match="contradict"):
        await Sandbox.create(ROOTFS, on="cloud", local=True)
    with pytest.raises(InvalidArgument, match="contradict"):
        await Sandbox.create(ROOTFS, on="local", local=False)
    with pytest.raises(InvalidArgument, match="contradict"):
        await Sandbox.create(ROOTFS, on="local", cloud=CloudOptions(warm=True))
    with pytest.raises(InvalidArgument, match="contradict"):
        await Sandbox.create(ROOTFS, local=True, cloud=CloudOptions(warm=True))
    assert not local_sdk
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, on="local", local=True, telemetry_enabled=False)


# ── kind and runtime reach the SDK ───────────────────────────────────────


async def test_local_kind_and_runtime_reach_the_sdk(local_sdk):
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, on="local", runtime="runc", telemetry_enabled=False)
    with pytest.raises(_Captured):
        await Sandbox.create(Image.linux(), on="local", runtime="qemu", telemetry_enabled=False)
    with pytest.raises(_Captured):
        await Sandbox.create(Image.linux(), on="local", kind="vm", telemetry_enabled=False)
    runc, qemu, vm = local_sdk
    assert (runc.on, runc.kind, runc.runtime) == ("local", "container", "runc")
    assert runc.image.startswith("container:")
    assert (qemu.kind, qemu.runtime) == ("vm", "qemu"), "the engine implies its kind"
    assert qemu.image.startswith("vm:")
    assert (vm.kind, vm.runtime) == ("vm", None) and vm.image.startswith("vm:")


async def test_ephemeral_takes_the_same_placement(local_sdk):
    with pytest.raises(_Captured):
        async with Sandbox.ephemeral(ROOTFS, on="local", runtime="gvisor", telemetry_enabled=False):
            pass
    assert (local_sdk[0].kind, local_sdk[0].runtime) == ("container", "gvisor")


# ── InvalidPlacement ─────────────────────────────────────────────────────


async def test_invalid_combinations_list_the_valid_values(local_sdk):
    with pytest.raises(InvalidPlacement, match="valid runtime: auto, gvisor, runc"):
        await Sandbox.create(ROOTFS, on="local", kind="container", runtime="qemu")
    with pytest.raises(InvalidPlacement, match="not available locally") as info:
        await Sandbox.create(ROOTFS, on="local", runtime="kubevirt")
    assert isinstance(info.value, InvalidArgument)
    with pytest.raises(InvalidPlacement, match="valid kind: auto, container, vm"):
        await Sandbox.create(ROOTFS, on="local", kind="pod")
    # The image's own kind against the engine.
    with pytest.raises(InvalidPlacement):
        await Sandbox.create(Image.linux(kind="vm"), on="local", runtime="runc")
    assert not local_sdk


async def test_an_sdk_placement_error_surfaces_as_invalid_placement(monkeypatch):
    n = cua._native

    class _Sandboxes:
        async def create(self, options):
            raise n.CuaError.InvalidPlacement("invalid placement: nope; valid runtime: auto")

    monkeypatch.setattr(
        native_rt, "local_runtime", lambda: SimpleNamespace(sandboxes=lambda: _Sandboxes())
    )
    with pytest.raises(InvalidPlacement, match="valid runtime: auto"):
        await Sandbox.create(ROOTFS, on="local", telemetry_enabled=False)


# ── the legacy Runtime object ────────────────────────────────────────────


class _Recorder(DockerRuntime):
    def __init__(self, calls: list) -> None:
        super().__init__()
        self.calls = calls

    async def start(self, image: Image, name: str, **opts: Any) -> Any:
        self.calls.append((image, opts))
        raise _Captured()


async def test_a_runtime_object_still_runs_locally(monkeypatch, notice):
    calls: list = []
    monkeypatch.setenv("CUA_DEFAULT_ON", "cloud")
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, runtime=_Recorder(calls), telemetry_enabled=False)
    # Shipped in 0.8.0: local=False with a Runtime object ran it locally.
    with pytest.raises(_Captured):
        await Sandbox.create(ROOTFS, runtime=_Recorder(calls), local=False, telemetry_enabled=False)
    assert len(calls) == 2 and not notice()
    for _, opts in calls:
        assert "kind" not in opts and "runtime" not in opts, "the adapter decides"
    with pytest.raises(InvalidArgument, match="on this machine"):
        await Sandbox.create(ROOTFS, runtime=_Recorder(calls), on="cloud")


# ── listing ──────────────────────────────────────────────────────────────


async def test_list_takes_location(monkeypatch):
    from cua_sandbox._ended import CloudEnded
    from cua_sandbox.sandbox import SandboxInfo

    async def local_rows(cls):
        return [SandboxInfo(name="a", status="running", source="container", location="local")]

    monkeypatch.setattr(Sandbox, "_list_local", classmethod(local_rows))
    assert [r.id for r in await Sandbox.list(location="local")] == ["local:a"]
    assert [r.id for r in await Sandbox.list(local=True)] == ["local:a"]
    with pytest.raises(CloudEnded):
        await Sandbox.list(location="cloud")
    with pytest.raises(InvalidArgument, match="contradict"):
        await Sandbox.list(location="cloud", local=True)
    with pytest.raises(InvalidArgument):
        await Sandbox.list(location="direct")
