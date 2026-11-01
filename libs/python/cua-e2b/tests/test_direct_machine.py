"""`CUA_E2B_ON=direct:<url>`: every sandbox is the same machine, one Space."""

import asyncio
import types

import pytest
from e2b import _backend


class FakeSpaces:
    def __init__(self):
        self.added, self.deleted, self.lifetimes = [], [], []

    async def add(self, url, token, name):
        self.added.append(name)
        return types.SimpleNamespace(id="direct:1.2.3.4:5", name=name)

    async def space(self, ref):
        return types.SimpleNamespace(spacesd=lambda: object())

    async def delete(self, ref):
        self.deleted.append(ref)

    async def set_lifetime(self, ref, secs, on_expire):
        self.lifetimes.append((ref, secs))

    async def list(self):
        # the registry knows the machine under the latest name only
        return [
            types.SimpleNamespace(
                id="direct:1.2.3.4:5", name=self.added[-1], power_state="", created_at_unix=1
            )
        ]


class FakeClient:
    def __init__(self):
        self.s = FakeSpaces()

    def spaces(self):
        return self.s

    def sandboxes(self):
        return types.SimpleNamespace(list=lambda loc: _async([]))


async def _async(v):
    return v


@pytest.fixture
def backend(monkeypatch, tmp_path):
    monkeypatch.setenv("CUA_HOME", str(tmp_path))
    monkeypatch.setenv("CUA_E2B_ON", "direct:http://1.2.3.4:5")
    monkeypatch.setenv("CUA_E2B_TOKEN", "t")
    b = _backend.CuaBackend(client=FakeClient())
    return b


async def test_killing_one_sandbox_keeps_the_machine_for_the_others(backend):
    a = await backend.create(None)
    b = await backend.create(None)
    c = backend._c.s
    assert await backend.state(a.sandbox_id) == "running"  # not the registry's latest name
    assert await a.delete() is True
    assert c.deleted == [] and await backend.state(b.sandbox_id) == "running"
    assert await b.delete() is True
    assert c.deleted == ["direct:1.2.3.4:5"]  # the last one removes the Space
    assert await b.delete() is False


async def test_a_direct_sandbox_never_puts_a_lifetime_on_the_shared_space(backend):
    a = await backend.create(None)
    assert await a.set_lifetime(5, "delete") is False  # the caller keeps the time
    assert backend._c.s.lifetimes == []


async def test_direct_sandboxes_are_listed_and_connectable_by_their_own_names(backend):
    a = await backend.create(None)
    b = await backend.create(None)
    names = {m.sandbox_id for m in await backend.list()}
    assert {a.sandbox_id, b.sandbox_id} <= names
    assert (await backend.connect(a.sandbox_id)).sandbox_id == a.sandbox_id
    assert await backend.kill(a.sandbox_id) is True
    assert await backend.kill(a.sandbox_id) is False
    assert asyncio.get_running_loop()


async def test_a_kill_racing_a_create_does_not_remove_the_machine_under_it(backend):
    slow = backend._c.s.add

    async def add(url, token, name):
        await asyncio.sleep(0.05)
        return await slow(url, token, name)

    backend._c.s.add = add
    a = await backend.create(None)
    b_task = asyncio.create_task(backend.create(None))
    await asyncio.sleep(0.01)  # the second create is between its claim and its add
    await a.delete()
    b = await b_task
    assert backend._c.s.deleted == []
    assert await backend.state(b.sandbox_id) == "running"


async def test_listing_orders_direct_sandboxes_by_their_own_start_not_the_spaces(backend):
    a = await backend.create(None)
    await asyncio.sleep(0.02)
    b = await backend.create(None)
    got = [
        m.sandbox_id for m in await backend.list() if m.sandbox_id in (a.sandbox_id, b.sandbox_id)
    ]
    assert sorted(got) == sorted([a.sandbox_id, b.sandbox_id]) and len(got) == 2
    started = {m.sandbox_id: m.started_at for m in await backend.list()}
    assert started[a.sandbox_id] < started[b.sandbox_id]
