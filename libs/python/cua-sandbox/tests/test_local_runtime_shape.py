"""Sandbox.create/ephemeral(local=True, cpu=, memory_mb=) size the auto-selected runtime.

Hermetic: runtime classes are replaced by recorders whose start() raises, so
no container or VM is ever launched.
"""

from __future__ import annotations

import pytest
from cua_sandbox import Image, Sandbox
from cua_sandbox.sandbox import _auto_runtime


class _Stop(Exception):
    pass


def _recorder(calls: list):
    class Recorder:
        def __init__(self, *args, **kwargs):
            calls.append(kwargs)

        async def start(self, *args, **kwargs):
            raise _Stop()

    return Recorder


@pytest.mark.asyncio
async def test_ephemeral_local_forwards_cpu_and_memory_to_docker(monkeypatch):
    calls: list = []
    monkeypatch.setattr("cua_sandbox.runtime.docker.DockerRuntime", _recorder(calls))

    with pytest.raises(_Stop):
        async with Sandbox.ephemeral(
            Image.linux(kind="container"),
            local=True,
            cpu=3,
            memory_mb=2048,
            telemetry_enabled=False,
        ):
            pass

    assert calls == [{"ephemeral": True, "cpus": 3, "memory_mb": 2048}]


@pytest.mark.asyncio
async def test_create_local_forwards_cpu_and_memory_to_lume(monkeypatch):
    calls: list = []
    monkeypatch.setattr("cua_sandbox.runtime.lume.LumeRuntime", _recorder(calls))

    with pytest.raises(_Stop):
        await Sandbox.create(
            Image.macos(), local=True, cpu=6, memory_mb=8192, telemetry_enabled=False
        )

    assert calls == [{"cpus": 6, "memory_mb": 8192}]


def test_auto_runtime_vm_shape_and_defaults(monkeypatch):
    calls: list = []
    monkeypatch.setattr("cua_sandbox.runtime.qemu.QEMURuntime", lambda **kw: calls.append(kw))
    monkeypatch.setattr("cua_sandbox.runtime.compat._has_qemu", lambda: True)

    _auto_runtime(Image.linux(kind="vm"), cpu=4, memory_mb=4096)
    _auto_runtime(Image.linux(kind="vm"))

    assert calls == [
        {"mode": "bare-metal", "cpu_count": 4, "memory_mb": 4096},
        {"mode": "bare-metal"},
    ]
