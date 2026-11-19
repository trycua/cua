"""The per-sandbox records file under many processes."""

import json
import multiprocessing
import os

from e2b import _backend


def _worker(home, n, i):
    os.environ["CUA_HOME"] = home
    for k in range(n):
        _backend._update_record(f"e2b-{i}-{k}", metadata={"w": str(i)}, started_at="t")
        _backend._update_record(f"e2b-{i}-{k}", resume="cold")


def test_concurrent_processes_lose_no_update(tmp_path, monkeypatch):
    monkeypatch.setenv("CUA_HOME", str(tmp_path))
    ctx = multiprocessing.get_context("spawn")
    procs = [ctx.Process(target=_worker, args=(str(tmp_path), 25, i)) for i in range(6)]
    for p in procs:
        p.start()
    for p in procs:
        p.join(60)
        assert p.exitcode == 0
    records = json.loads((tmp_path / "e2b-sandboxes.json").read_text())
    assert len(records) == 6 * 25
    assert all(r["resume"] == "cold" and r["metadata"] for r in records.values())


def test_a_field_set_to_none_is_dropped(tmp_path, monkeypatch):
    monkeypatch.setenv("CUA_HOME", str(tmp_path))
    _backend._update_record("e2b-a", resume="cold", metadata={"a": "1"})
    _backend._update_record("e2b-a", resume=None)
    assert _backend._load_records()["e2b-a"] == {"metadata": {"a": "1"}}


def test_the_cua_backend_client_can_be_opened_from_two_event_loops(monkeypatch):
    """The sync API runs on its own loop thread; the backend is shared."""
    import asyncio
    import threading

    backend = _backend.CuaBackend(client=None)
    opened = []

    async def fake_open(cua, address):
        opened.append(1)
        await asyncio.sleep(0.05)
        return object()

    monkeypatch.setattr(backend, "_daemon_or_embedded", fake_open)
    monkeypatch.setenv("CUA_E2B_MODE", "auto")
    import sys
    import types

    monkeypatch.setitem(sys.modules, "cua", types.ModuleType("cua"))
    results = []

    def other_loop():
        results.append(asyncio.run(backend.client()))

    t = threading.Thread(target=other_loop)
    t.start()
    results.append(asyncio.run(backend.client()))
    t.join()
    assert len(results) == 2 and results[0] is results[1]
