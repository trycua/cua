"""sandbox_create / sandbox_destroy telemetry never carries user-chosen names.

Pure unit tests: record_event is replaced with a recorder and no sandbox,
container or VM is created.
"""

from __future__ import annotations

import inspect
import sys
import time
from types import SimpleNamespace

import cua_sandbox.sandbox  # noqa: F401  (ensure the module is loaded)
import pytest

sandbox_mod = sys.modules["cua_sandbox.sandbox"]


@pytest.fixture
def recorded(monkeypatch):
    events: list[tuple[str, dict]] = []
    monkeypatch.setattr(sandbox_mod, "_TELEMETRY_AVAILABLE", True)
    monkeypatch.setattr(sandbox_mod, "is_telemetry_enabled", lambda: True)
    monkeypatch.setattr(sandbox_mod, "record_event", lambda n, p=None: events.append((n, p or {})))
    return events


def _fake_sandbox(runtime):
    return SimpleNamespace(name="alice-private-sandbox", telemetry_enabled=True, _runtime=runtime)


def test_create_event_has_no_name_and_coarse_runtime(recorded):
    from cua_sandbox.runtime.docker import DockerRuntime

    runtime = DockerRuntime.__new__(DockerRuntime)
    image = SimpleNamespace(os_type="linux", kind="container")
    sandbox_mod._record_sandbox_create(
        _fake_sandbox(runtime), image=image, local=True, ephemeral=False, t_start=time.monotonic()
    )
    ((name, props),) = recorded
    assert name == "sandbox_create"
    assert "name" not in props
    assert "alice-private-sandbox" not in repr(props)
    assert props["runtime_type"] == "docker"
    assert props["os_type"] == "linux"
    assert props["image_kind"] == "container"


def test_user_defined_runtime_is_other(recorded):
    class AlicesSecretRuntime:
        pass

    class DockerRuntime:  # same name as a built-in, but user-defined
        pass

    image = SimpleNamespace(os_type="alice-os", kind="weird")
    for rt in (AlicesSecretRuntime(), DockerRuntime()):
        sandbox_mod._record_sandbox_create(
            _fake_sandbox(rt), image=image, local=True, ephemeral=True, t_start=time.monotonic()
        )
    for _, props in recorded:
        assert props["runtime_type"] == "other"
        assert props["os_type"] == "other"
        assert props["image_kind"] == "other"
        assert "Alices" not in repr(props)


def test_destroy_event_has_no_name():
    src = inspect.getsource(sandbox_mod.Sandbox)
    line = next(ln for ln in src.splitlines() if '"sandbox_destroy"' in ln)
    assert "self.name" not in line
    assert '"name"' not in line
