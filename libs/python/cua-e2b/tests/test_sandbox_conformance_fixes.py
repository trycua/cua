"""Sandbox-level behaviours pinned against the E2B goldens (sbx_* conformance
cases): the 400/404 texts of E2B's API for malformed arguments, the not-found
and paused states, listing and metrics rules. In-memory backend."""

from __future__ import annotations

import asyncio
import dataclasses
import json
from datetime import datetime, timedelta, timezone

import e2b
import pytest
from e2b import AsyncSandbox, InvalidArgumentException, SandboxException
from e2b._backend import CuaMachine, set_backend
from fake_backend import FakeBackend

VALIDATION = (
    "400: validation error: error in openapi3filter.RequestError: request body has an error: "
    "doesn't match schema #/components/schemas/"
)


@pytest.fixture
def backend():
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


async def _sb(**kw):
    return await AsyncSandbox.create(template="w", **kw)


async def _state(sb) -> str:
    try:
        return str((await AsyncSandbox.get_info(sb.sandbox_id)).state)
    except e2b.SandboxNotFoundException:
        return "gone"


def _text(e: pytest.ExceptionInfo) -> str:
    return str(e.value)


# ── create: the request body is checked as E2B's API checks it ────────────────


@pytest.mark.parametrize(
    "kw, pointer, what",
    [
        ({"timeout": -1}, "/timeout", "number must be at least 1"),
        ({"timeout": 0}, "/timeout", "number must be at least 1"),
        ({"timeout": 30.5}, "/timeout", "value must be an integer"),
        ({"timeout": "30"}, "/timeout", "value must be an integer"),
        ({"metadata": {"k": 1}}, "/metadata/k", "value must be a string"),
        ({"envs": {"A": 1}}, "/envVars/A", "value must be a string"),
        ({"allow_internet_access": "no"}, "/allow_internet_access", "value must be a boolean"),
        ({"network": {"deny_out": [1]}}, "/network/denyOut/0", "value must be a string"),
        ({"mcp": "github"}, "/mcp", "value must be an object"),
        (
            {"lifecycle": {"on_timeout": "pause", "auto_resume": "yes"}},
            "/autoResume/enabled",
            "value must be a boolean",
        ),
    ],
)
async def test_create_refuses_what_the_api_schema_refuses(backend, kw, pointer, what):
    with pytest.raises(SandboxException) as e:
        await _sb(**kw)
    assert type(e.value) is SandboxException
    assert _text(e) == f'{VALIDATION}NewSandbox: Error at "{pointer}": {what}'
    assert not backend.machines


async def test_create_accepts_what_the_api_accepts(backend):
    # an empty metadata key, a long value and an odd env name are the API's to allow
    await _sb(metadata={"": "v"}, envs={"A B": "1"})
    await _sb(metadata={"k": "v" * 5000}, timeout=1)


async def test_allowed_domains_need_a_deny_of_all_traffic(backend):
    text = (
        "400: When specifying allowed domains in allow out, you must include "
        "'ALL_TRAFFIC' in deny out to block all other traffic."
    )
    for allow in ("1.1.1.1", ["not an ip!!"], ["1.1.1.1/33"], [""]):
        with pytest.raises(SandboxException) as e:
            await _sb(network={"allow_out": allow})
        assert _text(e) == text
    await _sb(network={"allow_out": ["1.1.1.1"], "deny_out": [e2b.ALL_TRAFFIC]})


async def test_template_errors_read_like_the_api(backend):
    from fake_backend import NotFound

    backend.fail_create = NotFound("image not found")
    with pytest.raises(SandboxException) as e:
        await AsyncSandbox.create(template="no-such")
    assert type(e.value) is SandboxException
    assert _text(e) == "404: template 'no-such' not found"
    backend.fail_create = None
    with pytest.raises(SandboxException) as e:
        await AsyncSandbox.create(template="Not A Template!!")
    assert _text(e) == "400: Invalid template reference: invalid template ID: Not A Template!!"


async def test_a_failure_that_is_not_a_missing_template_keeps_its_text(backend):
    backend.fail_create = SandboxException("qemu failed: Address already in use")
    with pytest.raises(SandboxException, match="Address already in use"):
        await _sb()


async def test_allow_internet_access_is_none_until_chosen(backend):
    assert (await (await _sb()).get_info()).allow_internet_access is None
    assert (await (await _sb(allow_internet_access=True)).get_info()).allow_internet_access is True
    assert (
        await (await _sb(allow_internet_access=False)).get_info()
    ).allow_internet_access is False


async def test_sandbox_has_no_envd_token(backend):
    sb = await _sb()
    assert sb._envd_access_token is None and sb.traffic_access_token is None


# ── set_timeout ───────────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    "value, what",
    [
        (-5, "number must be at least 0"),
        (1.5, "value must be an integer"),
        ("10", "value must be an integer"),
        (True, "value must be an integer"),
        (None, "Value is not nullable"),
    ],
)
async def test_set_timeout_edges(backend, value, what):
    sb = await _sb()
    for call in (sb.set_timeout(value), AsyncSandbox.set_timeout(sb.sandbox_id, value)):
        with pytest.raises(SandboxException) as e:
            await call
        assert _text(e) == f'{VALIDATION}SandboxTimeoutRequest: Error at "/timeout": {what}'
    assert await _state(sb) == "running"


async def test_set_timeout_zero_ends_the_sandbox_and_a_huge_one_is_capped(backend):
    sb = await _sb()
    await sb.set_timeout(10**9)
    assert (await sb.get_info()).end_at - datetime.now(timezone.utc) < timedelta(hours=25)
    await sb.set_timeout(0)
    assert await _state(sb) == "gone"
    with pytest.raises(e2b.SandboxNotFoundException, match="^Sandbox e2b-0 not found$"):
        await sb.get_info()


# ── unknown, malformed and killed ids ─────────────────────────────────────────


async def test_unknown_ids_read_like_the_api(backend):
    gone = "e2b-ffffffffff"
    for call, text in (
        (AsyncSandbox.connect(gone), f"Paused sandbox {gone} not found"),
        (AsyncSandbox.get_info(gone), f"Sandbox {gone} not found"),
        (AsyncSandbox.pause(gone), f"Sandbox {gone} not found"),
        (AsyncSandbox.set_timeout(gone, 60), f"Sandbox {gone} not found"),
    ):
        with pytest.raises(e2b.SandboxNotFoundException) as e:
            await call
        assert _text(e) == text
    assert await AsyncSandbox.kill(gone) is False
    assert await AsyncSandbox.kill("") is False
    assert await AsyncSandbox.get_metrics(gone) == []


async def test_malformed_ids_are_a_400(backend):
    for call in (AsyncSandbox.connect("not a valid id!"), AsyncSandbox.kill("not a valid id!")):
        with pytest.raises(SandboxException) as e:
            await call
        assert type(e.value) is SandboxException
        assert _text(e) == "400: Invalid sandbox ID"


async def test_calls_after_kill(backend):
    sb = await _sb()
    sid = sb.sandbox_id
    assert await sb.kill() is True
    assert await sb.kill() is False
    assert await AsyncSandbox.kill(sid) is False
    assert await sb.is_running() is False
    assert await sb.get_metrics() == []
    for call in (sb.get_info(), sb.set_timeout(60), sb.pause()):
        with pytest.raises(e2b.SandboxNotFoundException, match=f"^Sandbox {sid} not found$"):
            await call
    with pytest.raises(e2b.SandboxNotRunningException):
        await sb.commands.run("echo x")


# ── paused sandboxes ──────────────────────────────────────────────────────────


async def test_paused_sandbox_behaviour(backend):
    sb = await _sb()
    sid = sb.sandbox_id
    assert await AsyncSandbox.pause(sid) is True
    # another handle pausing again says so, as does the first
    assert await AsyncSandbox.pause(sid) is False
    assert await sb.pause() is False
    assert (await sb.get_info()).state == e2b.SandboxState.PAUSED
    assert str((await AsyncSandbox.get_info(sid)).state) == "paused"
    assert await sb.is_running() is False
    assert await AsyncSandbox.get_metrics(sid) == []
    # the paused sandbox cannot run commands or have its lifetime set
    with pytest.raises(e2b.SandboxNotRunningException):
        await sb.commands.run("echo x")
    with pytest.raises(e2b.SandboxNotFoundException, match=f"^Sandbox {sid} not found$"):
        await AsyncSandbox.set_timeout(sid, 300)
    with pytest.raises(e2b.SandboxNotFoundException):
        await sb.set_timeout(300)
    listed = await AsyncSandbox.list(
        query=e2b.SandboxQuery(state=[e2b.SandboxState.PAUSED])
    ).next_items()
    assert [i.sandbox_id for i in listed] == [sid]
    # connect resumes it, and the first handle sees the sandbox running again
    again = await AsyncSandbox.connect(sid)
    assert (await again.commands.run("echo back")).stdout == "back\n"
    assert await _state(sb) == "running"
    assert (await sb.commands.run("echo mine")).stdout == "mine\n"
    assert await AsyncSandbox.kill(sid) is True


# ── metrics, fork ─────────────────────────────────────────────────────────────


async def test_metrics_windows(backend):
    sb = await _sb()
    now = datetime.now(timezone.utc)
    for kw in (
        {"start": now + timedelta(hours=1)},
        {"start": now, "end": now - timedelta(minutes=10)},
    ):
        for call in (sb.get_metrics(**kw), AsyncSandbox.get_metrics(sb.sandbox_id, **kw)):
            with pytest.raises(SandboxException) as e:
                await call
            assert _text(e) == "400: Invalid time range for metrics"
    assert len(await sb.get_metrics(start=now - timedelta(minutes=10), end=now)) == 1


@pytest.mark.parametrize("count", [0, -1, 21, True, "2", 1.5])
async def test_fork_count_is_validated_before_anything_forks(backend, count):
    sb = await _sb()
    for call in (sb.fork(count=count), AsyncSandbox.fork(sb.sandbox_id, count=count)):
        with pytest.raises(InvalidArgumentException) as e:
            await call
        assert _text(e) == "count must be an integer between 1 and 20"
    assert len(backend.machines) == 1


# ── listing ───────────────────────────────────────────────────────────────────


async def test_listing_limits_order_and_tokens(backend):
    made = [await _sb(metadata={"n": str(i)}) for i in range(3)]
    for i, sb in enumerate(made):
        backend.records[sb.sandbox_id]["started_at"] = datetime(
            2026, 1, 1 + i, tzinfo=timezone.utc
        ).isoformat()
    # newest first unless the order is given
    default = [i.sandbox_id for i in await AsyncSandbox.list().next_items()]
    assert default == [s.sandbox_id for s in reversed(made)]
    asc = [i.sandbox_id for i in await AsyncSandbox.list(order="asc").next_items()]
    assert asc == [s.sandbox_id for s in made]
    assert len(await AsyncSandbox.list(limit=0).next_items()) == 3
    for limit, what in ((-1, "number must be at least 1"), (100000, "number must be at most 100")):
        with pytest.raises(SandboxException) as e:
            await AsyncSandbox.list(limit=limit).next_items()
        assert _text(e) == (
            f'400: validation error: error in openapi3filter.RequestError: parameter "limit" in query has an error: {what}'
        )
    with pytest.raises(SandboxException) as e:
        await AsyncSandbox.list(next_token="garbage-token").next_items()
    assert _text(e) == "400: Invalid next token"
    # an invalid order is the SDK's own error, raised when the page is fetched
    paginator = AsyncSandbox.list(order="sideways")
    with pytest.raises(InvalidArgumentException, match="Invalid order 'sideways'"):
        await paginator.next_items()


async def test_a_listed_sandbox_reports_what_get_info_does(backend):
    sb = await _sb(timeout=600, metadata={"a": "b"}, network={"deny_out": ["10.0.0.0/8"]})
    backend.records[sb.sandbox_id]["started_at"] = "2026-01-01T00:00:00+00:00"
    backend.records[sb.sandbox_id]["end_at"] = "2026-01-01T00:10:00+00:00"
    listed = (await AsyncSandbox.list().next_items())[0]
    info = await AsyncSandbox.get_info(sb.sandbox_id)
    assert dataclasses.replace(listed, started_at=info.started_at) == info
    assert listed.end_at == datetime(2026, 1, 1, 0, 10, tzinfo=timezone.utc)  # the recorded end
    assert listed.network == {"deny_out": ["10.0.0.0/8"], "allow_public_traffic": True}
    assert listed.metadata == {"a": "b"}


async def test_snapshot_ids_are_checked_like_template_ids(backend):
    with pytest.raises(SandboxException) as e:
        await AsyncSandbox.delete_snapshot("!! bad")
    assert _text(e) == "400: Invalid template ID: invalid template ID: !! bad"
    assert await AsyncSandbox.delete_snapshot("conf/no-such-snapshot:v0") is False


# ── network updates ───────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    "update, text",
    [
        ({"deny_out": ["example.com"]}, "400: invalid denied CIDR example.com"),
        ({"deny_out": ["999.1.1.1"]}, "400: invalid denied CIDR 999.1.1.1"),
        ({"deny_out": ["1.1.1.1/33"]}, "400: invalid denied CIDR 1.1.1.1/33"),
        ({"deny_out": [""]}, "400: invalid denied CIDR "),
        (
            {"deny_out": [1]},
            f'{VALIDATION}SandboxNetworkUpdateConfig: Error at "/denyOut/0": value must be a string',
        ),
    ],
)
async def test_network_updates_are_checked_like_e2bs_api(backend, update, text):
    sb = await _sb(network={"allow_out": ["1.1.1.1"], "deny_out": [e2b.ALL_TRAFFIC]})
    before = dict(backend.machines[sb.sandbox_id].egress)
    for call in (sb.update_network(update), AsyncSandbox.update_network(sb.sandbox_id, update)):
        with pytest.raises(SandboxException) as e:
            await call
        assert type(e.value) is SandboxException and _text(e) == text
    assert backend.machines[sb.sandbox_id].egress == before  # the sandbox keeps what it had


async def test_network_update_accepts_unknown_keys_and_refuses_none(backend):
    sb = await _sb()
    await sb.update_network({"bogus": []})
    with pytest.raises(AttributeError):
        await sb.update_network(None)


async def test_info_reports_network_as_e2b_does(backend):
    sb = await _sb(network={"allow_out": ["1.1.1.1"], "deny_out": [e2b.ALL_TRAFFIC]})
    assert (await sb.get_info()).network == {
        "allow_out": ["1.1.1.1"],
        "deny_out": ["0.0.0.0/0"],
        "allow_public_traffic": True,
    }
    await sb.update_network({"deny_out": ["1.0.0.0/24"]})
    assert (await sb.get_info()).network == {
        "deny_out": ["1.0.0.0/24"],
        "allow_public_traffic": True,
    }
    again = await AsyncSandbox.connect(sb.sandbox_id)
    assert (await again.get_info()).network == (await sb.get_info()).network


# ── signed URLs ───────────────────────────────────────────────────────────────


async def test_signed_url_of_a_killed_sandbox_is_not_found(backend):
    sb = await _sb()
    sb.upload_url("/tmp/x")
    backend.machines[sb.sandbox_id].deleted = True
    await sb.kill()
    with pytest.raises(e2b.SandboxNotFoundException):
        sb.upload_url("/tmp/x")


class _Env:
    def __init__(self, url_path: str):
        self.url_path = url_path
        self.requests = []

    async def call_json(self, method, body):
        self.requests.append(json.loads(body))
        return json.dumps({"urlPath": self.url_path})

    def endpoint(self):
        return "http://10.0.0.5:3211"


async def _machine(env) -> CuaMachine:
    m = CuaMachine(None, "e2b-x", env)
    m._driver_user = "cua"
    return m


async def test_signed_url_for_another_user_needs_spacesd_run_as(backend):
    old = _Env("/files?path=%2Ftmp%2Fa&method=GET&exp=1&sig=s")
    m = await _machine(old)
    with pytest.raises(InvalidArgumentException, match="cannot sign file URLs for user='root'"):
        await m.signed_url("/tmp/a", "GET", 60, "root")
    assert old.requests[0]["runAs"] == "root"
    new = _Env("/files?path=%2Ftmp%2Fa&method=GET&exp=1&ru=root&sig=s")
    m = await _machine(new)
    assert (await m.signed_url("/tmp/a", "GET", 60, "root")).endswith("&ru=root&sig=s")
    # the default user needs no run_as, and an upload may create the parent directories
    m = await _machine(_Env("/files?path=%2Ftmp%2Fa&method=PUT&exp=1&pp=1&sig=s"))
    await m.signed_url("/tmp/a", "PUT", 60, "cua")
    assert m._env.requests[0].get("runAs") is None and m._env.requests[0]["createParents"] is True


async def test_expiry_does_not_outlive_the_wait(backend):
    # the lifetime timer of an embedded runtime still ends a sandbox on time
    sb = await _sb()
    await sb._set_lifetime(0.02)
    await asyncio.sleep(0.08)
    assert await _state(sb) == "gone"
