"""BenchAdapter waits for the benchmark server's health path before setup."""

import asyncio
from types import SimpleNamespace

import pytest

from cua_bench.adapters import BenchAdapter, ServerSpec


class FakeService:
    def __init__(self, statuses):
        self.statuses = list(statuses)
        self.calls = []

    async def request(self, method, path, **kwargs):
        self.calls.append((method, path))
        status = self.statuses.pop(0) if self.statuses else 200
        if isinstance(status, Exception):
            raise status
        return SimpleNamespace(status_code=status)


class FakeSandbox:
    def __init__(self, service):
        self._service = service

    def service(self, name):
        assert name == "server"
        return self._service


class Web(BenchAdapter):
    id, version = "web", "1"
    server = ServerSpec(port=7000, health="/healthz")


def session_with(statuses):
    svc = FakeService(statuses)
    return SimpleNamespace(sandbox=FakeSandbox(svc)), svc


def test_waits_until_healthy(monkeypatch):
    monkeypatch.setattr(asyncio, "sleep", _no_sleep)
    session, svc = session_with([ConnectionError("refused"), 503, 200])
    asyncio.run(Web()._wait_healthy(Web().endpoints(session)))
    assert svc.calls == [("GET", "/healthz")] * 3


def test_times_out_with_the_last_status(monkeypatch):
    monkeypatch.setattr(asyncio, "sleep", _no_sleep)
    adapter = Web()
    adapter.health_timeout = 0.0
    session, _ = session_with([503] * 5)
    with pytest.raises(TimeoutError, match="not healthy"):
        asyncio.run(adapter._wait_healthy(adapter.endpoints(session)))


def test_no_server_or_no_sandbox_is_a_no_op():
    class NoServer(BenchAdapter):
        id, version = "x", "1"

    asyncio.run(NoServer()._wait_healthy(NoServer().endpoints(SimpleNamespace(sandbox=None))))
    asyncio.run(Web()._wait_healthy(Web().endpoints(SimpleNamespace())))


_real_sleep = asyncio.sleep


async def _no_sleep(_s):
    await _real_sleep(0)
