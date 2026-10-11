"""``network=``: local guests get outbound network by default (like a Docker
container); ``network="none"`` cuts it. Hermetic: nothing boots.

The native SDK path is checked by capturing the options handed to the SDK;
the legacy in-process QEMU launcher (OSWorld disks, ISOs, QMP transport,
extra args) by the command line it builds.
"""

from __future__ import annotations

import asyncio
import importlib
import json
from types import SimpleNamespace

import pytest
from cua_sandbox import Image, Sandbox, sandbox_state
from cua_sandbox._sdk import Unsupported
from cua_sandbox.options import check_network
from cua_sandbox.runtime import native as native_module
from cua_sandbox.runtime import qemu as qemu_module
from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime, _netdev

sandbox_module = importlib.import_module("cua_sandbox.sandbox")


def test_check_network_normalizes_and_rejects_unknown_modes():
    assert check_network(None) is None
    assert check_network("default") == "default"
    assert check_network(" NONE ") == "none"
    assert check_network("") == "default"
    for bad in ("host", "bridge", 0):
        with pytest.raises(ValueError):
            check_network(bad)  # type: ignore[arg-type]


def test_netdev_restricts_only_on_request_and_keeps_the_forwards():
    fwd = "hostfwd=tcp:127.0.0.1:4000-:3211"
    assert _netdev(fwd) == f"user,id=net0,{fwd}"
    assert _netdev(fwd, restrict=True) == f"user,id=net0,restrict=on,{fwd}"


def test_android_cmd_has_egress_unless_restricted():
    rt = QEMUBaremetalRuntime()
    rt._qemu_bin = lambda: "qemu-system-x86_64"  # type: ignore[method-assign]
    args = ("a", "/d.qcow2", "qcow2", 2048, 2, 4000, 1, False, 4444)
    default = rt._build_android_cmd(*args)
    none = rt._build_android_cmd(*args, restrict=True)
    net = lambda cmd: cmd[cmd.index("-netdev") + 1]  # noqa: E731
    assert "restrict" not in net(default)
    assert net(none).startswith("user,id=net0,restrict=on,hostfwd=")


def test_cloud_sandboxes_refuse_network_none():
    with pytest.raises(Unsupported, match="network='none'"):
        asyncio.run(Sandbox.create(Image.linux(), local=False, network="none"))
    with pytest.raises(ValueError):
        asyncio.run(Sandbox.create(Image.linux(), local=True, network="host"))


class _Captured(Exception):
    pass


def _capture_native(monkeypatch) -> list:
    pytest.importorskip("cua")
    seen: list = []

    class _Sandboxes:
        async def create(self, options):
            seen.append(options)
            raise _Captured()

    monkeypatch.setattr(
        native_module, "local_runtime", lambda: SimpleNamespace(sandboxes=lambda: _Sandboxes())
    )
    monkeypatch.setattr(sandbox_module, "_TELEMETRY_AVAILABLE", False)
    # What reaches the SDK, independent of whether this host has QEMU.
    monkeypatch.setattr("cua_sandbox.runtime.compat._has_qemu", lambda: True)
    return seen


def test_network_reaches_the_native_sdk(monkeypatch):
    seen = _capture_native(monkeypatch)
    vm = Image.linux(kind="vm")
    with pytest.raises(_Captured):
        asyncio.run(Sandbox.create(vm, local=True))
    with pytest.raises(_Captured):
        asyncio.run(Sandbox.create(vm, local=True, network="none"))
    assert seen[0].network is None, "unset: outbound network"
    assert seen[1].network == "none"


def test_runtimes_that_cannot_cut_egress_refuse_network_none():
    class _Other:
        async def start(self, *a, **k):  # pragma: no cover - refused first
            raise AssertionError("must not start")

    with pytest.raises(Unsupported, match="network='none'"):
        asyncio.run(
            Sandbox.create(Image.linux(kind="vm"), local=True, runtime=_Other(), network="none")
        )


def _legacy_cmd(monkeypatch, tmp_path, **opts) -> list[str]:
    """The qemu argv the legacy launcher builds for an OSWorld disk."""
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    disk = tmp_path / "osworld.qcow2"
    disk.write_bytes(b"qcow2")
    seen: list[list[str]] = []

    def _run(cmd, **_):
        seen.append(cmd)
        return SimpleNamespace(returncode=0, stderr="")

    monkeypatch.setattr(qemu_module.subprocess, "run", _run)
    rt = QEMUBaremetalRuntime()
    rt._qemu_bin = lambda: "qemu-system-x86_64"  # type: ignore[method-assign]

    async def _ready(*_a, **_k):
        return True

    monkeypatch.setattr(rt, "is_ready", _ready)
    image = Image.from_file(str(disk), os_type="linux", agent_type="osworld")
    asyncio.run(rt.start(image, "cua-e2e-net", ephemeral=False, **opts))
    [cmd] = [c for c in seen if c and c[0] == "qemu-system-x86_64"]
    return cmd


def test_legacy_launcher_gives_egress_by_default(monkeypatch, tmp_path):
    cmd = _legacy_cmd(monkeypatch, tmp_path)
    net = cmd[cmd.index("-netdev") + 1]
    assert "restrict" not in net, net
    assert json.loads((tmp_path / "cua-e2e-net.json").read_text())["network"] == "default"


def test_legacy_launcher_restricts_with_network_none(monkeypatch, tmp_path):
    cmd = _legacy_cmd(monkeypatch, tmp_path, network="none")
    net = cmd[cmd.index("-netdev") + 1]
    assert net.startswith("user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:"), net
    assert json.loads((tmp_path / "cua-e2e-net.json").read_text())["network"] == "none"
