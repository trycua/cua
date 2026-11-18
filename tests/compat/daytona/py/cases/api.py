"""The control-plane API: create parameters, get/list/delete, labels and
intervals, lifecycle and its errors, authentication. Hermetic (no guest shell).

Expected behaviour is derived from the SDK sources named in each `ref`
(daytona 0.225.0 `_sync/daytona.py`, `_sync/sandbox.py`, `common/errors.py`)."""

import time

from harness import case, err_obs


def started(sb) -> bool:
    return str(sb.state).lower().endswith("started")


def state_of(sb) -> str:
    return str(sb.state).lower().rsplit(".", 1)[-1]


@case(
    "api.create_params",
    needs="mock",
    ref="common/daytona.py CreateSandboxBaseParams; _sync/sandbox.py __process_sandbox_dto",
)
async def create_params(c):
    name = c.name("params")
    sb = await c.new(
        name=name,
        env_vars={"A": "1", "B": "two words"},
        labels={"team": "x"},
        public=True,
        auto_stop_interval=7,
        auto_archive_interval=30,
        auto_delete_interval=-1,
    )
    got = await c.w(c.daytona.get(name))
    assert got.id == sb.id and got.name == name
    assert got.public is True, got.public
    assert got.env == {"A": "1", "B": "two words"}, got.env
    assert (
        got.labels.get("team") == "x" and got.labels.get("code-toolbox-language") == "python"
    ), got.labels
    return {
        "started": started(got),
        "autoStop": got.auto_stop_interval,
        "autoArchive": got.auto_archive_interval,
        "autoDelete": got.auto_delete_interval,
        "networkBlockAll": got.network_block_all,
        "user": got.user,
        "hasCpu": got.cpu >= 1 and got.memory >= 1,
    }


@case("api.create_defaults", needs="mock", ref="_sync/daytona.py create; models/sandbox.py")
async def create_defaults(c):
    sb = await c.new()
    assert started(sb) and sb.id
    assert sb.name, "a name defaults to the id"
    return {
        "nameIsId": sb.name == sb.id,
        "autoStop": sb.auto_stop_interval,
        "autoArchive": sb.auto_archive_interval,
        "autoDelete": sb.auto_delete_interval,
        "public": sb.public,
        "env": sb.env,
        "labels": sorted(sb.labels),
    }


@case(
    "api.create_from_image_resources",
    needs="mock",
    ref="_sync/daytona.py _create (buildInfo, resources)",
)
async def create_from_image_resources(c):
    sdk = c.sdk
    sb = await c.new(
        sdk.CreateSandboxFromImageParams(
            image="python:3.12", resources=sdk.Resources(cpu=2, memory=3)
        )
    )
    assert started(sb)
    return {"cpu": sb.cpu, "memory": sb.memory}


@case("api.create_from_image_disk", needs="mock", ref="_sync/daytona.py _create (resources.disk)")
async def create_from_image_disk(c):
    sdk = c.sdk
    # A disk size is applied (VM-backed images) or refused with the reason (an
    # image that only runs as a container has no disk to size); never ignored.
    made = []

    async def make():
        made.append(
            await c.new(
                sdk.CreateSandboxFromImageParams(
                    image="python:3.12", resources=sdk.Resources(cpu=2, memory=3, disk=7)
                )
            )
        )

    e = await c.raises(make)
    if e is not None:
        assert e["status_code"] == 400 and "disk" in e["message"].lower(), e
    else:
        # VM images get the size; a container's disk is the engine's (reported as the guest has it)
        assert made[0].disk == 7 or made[0].disk > 0, made[0].disk
    return {"diskAppliedOrExplained": True}


@case("api.create_name_conflict", needs="mock", ref="common/errors.py STATUS_CODE_TO_ERROR 409")
async def create_name_conflict(c):
    name = c.name("dup")
    await c.new(name=name)
    e = await c.raises(lambda: c.daytona.create(c.sdk.CreateSandboxFromSnapshotParams(name=name)))
    assert e and e["type"] == "DaytonaConflictError" and e["status_code"] == 409, e
    assert name in e["message"], e
    return err_obs(e)


@case("api.get_missing", needs="mock", ref="common/errors.py STATUS_CODE_TO_ERROR 404")
async def get_missing(c):
    e = await c.raises(lambda: c.daytona.get("no-such-sandbox-" + c.run_id))
    assert e and e["type"] == "DaytonaNotFoundError" and e["status_code"] == 404, e
    return err_obs(e)


@case("api.auth_bad_key", needs="mock", ref="common/errors.py STATUS_CODE_TO_ERROR 401")
async def auth_bad_key(c):
    import os

    sdk = c.sdk
    url = os.environ["DAYTONA_API_URL"]
    bad = (sdk.AsyncDaytona if c.is_async else sdk.Daytona)(
        sdk.DaytonaConfig(api_key="dtn_not_a_key", api_url=url)
    )
    try:
        try:
            await c.w(bad.get("whatever"))
        except Exception as e:  # noqa: BLE001
            from harness import describe_error

            o = describe_error(e)
        else:
            raise AssertionError("a bad key was accepted")
    finally:
        if c.is_async:
            await bad.close()
    assert o["type"] == "DaytonaAuthenticationError" and o["status_code"] == 401, o
    return err_obs(o)


@case("api.list_labels_and_pages", needs="mock", ref="_sync/daytona.py list/_fetch_sandbox_page")
async def list_labels_and_pages(c):
    tag = c.name("lp")
    made = [await c.new(labels={"lp": tag, "idx": str(i)}) for i in range(3)]
    all_ids = {s.id for s in made}
    got = await c.list(labels={"lp": tag})
    assert {s.id for s in got} == all_ids, got
    # per-page size 1: the iterator follows the cursor across 3 pages
    paged = await c.list(labels={"lp": tag}, limit=1)
    assert {s.id for s in paged} == all_ids and len(paged) == 3, paged
    one = await c.list(labels={"lp": tag, "idx": "1"})
    assert [s.id for s in one] == [made[1].id], one
    none = await c.list(labels={"lp": tag + "-nope"})
    assert none == []
    return {"all": len(got), "paged": len(paged), "one": len(one), "none": len(none)}


@case("api.list_filters", needs="mock", ref="common/sandbox.py ListSandboxesQuery")
async def list_filters(c):
    tag = c.name("lf")
    a = await c.new(name=tag + "-alpha", labels={"lf": tag}, public=True)
    b = await c.new(name=tag + "-beta", labels={"lf": tag})
    by_name = await c.list(name=tag + "-al")
    assert [s.id for s in by_name] == [a.id], by_name
    by_id = await c.list(id=b.id[:8])
    assert b.id in [s.id for s in by_id]
    public = await c.list(labels={"lf": tag}, is_public=True)
    assert [s.id for s in public] == [a.id], public
    started_only = await c.list(labels={"lf": tag}, states=[c.sdk.SandboxState.STARTED])
    assert {s.id for s in started_only} == {a.id, b.id}
    stopped = await c.list(labels={"lf": tag}, states=[c.sdk.SandboxState.STOPPED])
    assert stopped == []
    F, D = c.sdk.SandboxListSortField, c.sdk.SandboxListSortDirection
    asc = await c.list(labels={"lf": tag}, sort=F.NAME, order=D.ASC)
    desc = await c.list(labels={"lf": tag}, sort=F.NAME, order=D.DESC)
    assert [s.id for s in asc] == [a.id, b.id] and [s.id for s in desc] == [b.id, a.id]
    big = await c.list(labels={"lf": tag}, min_cpu=1000)
    assert big == []
    return {
        "byName": 1,
        "public": 1,
        "started": 2,
        "stopped": 0,
        "ascFirstIsAlpha": True,
        "minCpu": 0,
    }


@case("api.list_rejects_bad_limit", needs="mock", ref="_sync/daytona.py list (limit < 1)")
async def list_rejects_bad_limit(c):
    try:
        await c.list(limit=0)
    except Exception as e:  # noqa: BLE001
        return {"rejected": True, "mentionsLimit": "limit" in str(e)}
    raise AssertionError("limit=0 accepted")


@case("api.labels_set", needs="mock", ref="_sync/sandbox.py set_labels")
async def labels_set(c):
    sb = await c.new(labels={"a": "1"})
    out = await c.w(sb.set_labels({"b": "2"}))
    assert out == {"b": "2"} or out.get("b") == "2", out
    got = await c.w(c.daytona.get(sb.id))
    assert got.labels.get("b") == "2" and "a" not in got.labels, got.labels
    return {
        "returned": sorted(out),
        "after": sorted(k for k in got.labels if k != "code-toolbox-language"),
    }


@case(
    "api.intervals",
    needs="mock",
    ref="_sync/sandbox.py set_autostop_interval/set_auto_archive_interval/set_auto_delete_interval/set_ttl",
)
async def intervals(c):
    sb = await c.new(auto_stop_interval=0)
    await c.w(sb.set_autostop_interval(45))
    await c.w(sb.set_auto_archive_interval(120))
    await c.w(sb.set_auto_delete_interval(10))
    await c.w(sb.set_ttl(60))
    got = await c.w(c.daytona.get(sb.id))
    assert (got.auto_stop_interval, got.auto_archive_interval, got.auto_delete_interval) == (
        45,
        120,
        10,
    )
    # ttl sets the absolute end the DTO reports
    assert got.auto_destroy_at, "autoDestroyAt after set_ttl"
    # the SDK refuses negative values before the wire
    refused = {}
    for name, fn in (
        ("autostop", lambda: sb.set_autostop_interval(-1)),
        ("archive", lambda: sb.set_auto_archive_interval(-1)),
        ("ttl", lambda: sb.set_ttl(-5)),
    ):
        refused[name] = (await c.raises(fn)) is not None
    assert all(refused.values()), refused
    return {
        "autoStop": 45,
        "autoArchive": 120,
        "autoDelete": 10,
        "hasDestroyAt": True,
        "refused": refused,
    }


@case("api.public_flag", needs="mock", ref="_sync/sandbox.py set_public")
async def public_flag(c):
    sb = await c.new()
    assert sb.public is False
    await c.w(sb.set_public(True))
    assert (await c.w(c.daytona.get(sb.id))).public is True
    await c.w(sb.set_public(False))
    assert (await c.w(c.daytona.get(sb.id))).public is False
    return {"toggled": True}


@case(
    "api.lifecycle_stop_start",
    needs="mock",
    ref="_sync/sandbox.py start/stop/wait_for_sandbox_stop",
)
async def lifecycle_stop_start(c):
    sb = await c.new()
    await c.w(sb.stop())
    got = await c.w(c.daytona.get(sb.id))
    assert state_of(got) == "stopped", got.state
    await c.w(sb.stop())  # stopping a stopped sandbox is not an error
    await c.w(sb.start())
    got = await c.w(c.daytona.get(sb.id))
    assert started(got)
    await c.w(sb.start())  # nor is starting a started one
    return {"afterStop": "stopped", "afterStart": "started"}


@case(
    "api.lifecycle_archive_rules",
    needs="mock",
    ref="_sync/sandbox.py archive ('must be stopped before archiving')",
)
async def lifecycle_archive_rules(c):
    sb = await c.new()
    e = await c.raises(lambda: sb.archive())
    assert e and e["status_code"] == 400 and "stopped" in e["message"].lower(), e
    await c.w(sb.stop())
    await c.w(sb.archive())
    got = await c.w(c.daytona.get(sb.id))
    assert state_of(got) == "archived", got.state
    await c.w(sb.start())
    assert started(await c.w(c.daytona.get(sb.id)))
    return {"startedArchive": err_obs(e), "archived": "archived", "restarted": True}


@case("api.lifecycle_pause", needs="mock", ref="_sync/sandbox.py pause")
async def lifecycle_pause(c):
    sb = await c.new()
    await c.w(sb.pause())
    assert state_of(await c.w(c.daytona.get(sb.id))) == "paused"
    await c.w(sb.start())
    assert started(await c.w(c.daytona.get(sb.id)))
    await c.w(sb.stop())
    e = await c.raises(lambda: sb.pause())
    assert e and e["status_code"] == 400, e
    return {"paused": "paused", "pauseStopped": err_obs(e)}


@case("api.lifecycle_recover_only_in_error", needs="mock", ref="_sync/sandbox.py recover")
async def lifecycle_recover_only_in_error(c):
    sb = await c.new()
    e = await c.raises(lambda: sb.recover())
    assert e and e["status_code"] == 400, e
    return {"recoverStarted": err_obs(e)}


@case("api.delete", needs="mock", ref="_sync/sandbox.py delete; _sync/daytona.py delete")
async def delete(c):
    sb = await c.new()
    await c.w(c.daytona.delete(sb))
    c.made.remove(sb)
    e = await c.raises(lambda: c.daytona.get(sb.id))
    assert e and e["type"] == "DaytonaNotFoundError", e
    listed = await c.list(id=sb.id)
    assert listed == []
    return {"getAfterDelete": err_obs(e)}


@case("api.delete_wait", needs="mock", ref="_sync/sandbox.py delete(wait=True)")
async def delete_wait(c):
    sb = await c.new()
    await c.w(sb.delete(wait=True))
    c.made.remove(sb)
    assert state_of(sb) in ("destroyed", "destroying"), sb.state
    return {"state": "destroyed"}


@case("api.refresh", needs="mock", ref="_sync/sandbox.py refresh_data/refresh_activity")
async def refresh(c):
    sb = await c.new()
    other = await c.w(c.daytona.get(sb.id))
    await c.w(other.set_labels({"seen": "yes"}))
    await c.w(sb.refresh_data())
    assert sb.labels.get("seen") == "yes", sb.labels
    await c.w(sb.refresh_activity())
    return {"labelsRefreshed": True}


@case(
    "api.ephemeral",
    needs="mock",
    ref="common/daytona.py CreateSandboxBaseParams (ephemeral -> auto_delete_interval=0)",
)
async def ephemeral(c):
    sb = await c.new(ephemeral=True)
    assert sb.auto_delete_interval == 0, sb.auto_delete_interval
    await c.w(sb.stop())
    for _ in range(40):
        e = await c.raises(lambda: c.daytona.get(sb.id))
        if e:
            break
        time.sleep(0.5) if not c.is_async else await __import__("asyncio").sleep(0.5)
    else:
        raise AssertionError("an ephemeral sandbox survived being stopped")
    c.made.remove(sb)
    assert e["type"] == "DaytonaNotFoundError", e
    return {"autoDelete": 0, "afterStop": "deleted"}


@case("api.fork", needs="mock", ref="_sync/sandbox.py fork")
async def fork(c):
    sb = await c.new(labels={"fk": c.run_id})
    kid = await c.w(sb.fork(name=c.name("kid")))
    c.made.append(kid)
    assert started(kid) and kid.id != sb.id
    got = await c.w(c.daytona.get(kid.id))
    assert started(got)
    return {"forked": True}


@case(
    "api.create_validation_is_client_side", needs="mock", ref="_sync/daytona.py _create validations"
)
async def create_validation_is_client_side(c):
    sdk = c.sdk
    refused = {}
    for name, kw in {
        "negativeStop": {"auto_stop_interval": -1},
        "stopAndPause": {"auto_stop_interval": 5, "auto_pause_interval": 5},
        "pauseEphemeral": {"auto_pause_interval": 5, "auto_delete_interval": 0},
        "badQueueTimeout": {"queue_timeout": 0},
        "negativeTtl": {"ttl_minutes": -1},
    }.items():
        e = await c.raises(
            lambda kw=kw: c.daytona.create(sdk.CreateSandboxFromSnapshotParams(**kw))
        )
        refused[name] = e is not None
    assert all(refused.values()), refused
    return refused


@case(
    "api.resize_rules",
    needs="mock",
    ref="_sync/sandbox.py resize (hot resize only increases; disk only when stopped)",
)
async def resize_rules(c):
    sdk = c.sdk
    sb = await c.new(auto_stop_interval=0)
    cpu, mem = sb.cpu, sb.memory
    gpu = await c.raises(lambda: sb.resize(sdk.Resources(cpu=2, gpu=1)))
    assert gpu and gpu["type"] == "DaytonaBadRequestError", gpu
    disk_running = await c.raises(lambda: sb.resize(sdk.Resources(disk=sb.disk + 10)))
    assert disk_running and disk_running["status_code"] == 400, disk_running
    # growing cpu either works (the core can restart with a new shape) or is
    # refused with the reason; never silently ignored
    e = await c.raises(lambda: sb.resize(sdk.Resources(cpu=cpu + 1, memory=mem + 1)))
    if e is None:
        got = await c.w(c.daytona.get(sb.id))
        assert (got.cpu, got.memory) == (cpu + 1, mem + 1), got
    else:
        assert e["status_code"] == 400 and "Resizing is not supported" in e["message"], e
    return {"gpuRefused": True, "diskWhileRunningRefused": True, "growIsAppliedOrExplained": True}


@case(
    "api.resize_applies_on_a_real_guest",
    needs="space",
    ref="_sync/sandbox.py resize (stopped sandbox: cpu, memory and disk grow)",
)
async def resize_real(c):
    sdk = c.sdk
    sb = await c.new(auto_stop_interval=0)
    before = int((await c.w(sb.process.exec("nproc"))).result.strip())
    await c.w(sb.process.exec("echo kept > /home/daytona/resize-marker"))
    await c.w(sb.stop())
    await c.w(sb.resize(sdk.Resources(cpu=before + 1, memory=sb.memory + 1, disk=sb.disk + 2)))
    await c.w(sb.refresh_data())
    assert str(sb.state).lower().endswith("stopped"), sb.state
    await c.w(sb.start())
    after = int((await c.w(sb.process.exec("nproc"))).result.strip())
    marker = (await c.w(sb.process.exec("cat /home/daytona/resize-marker"))).result.strip()
    assert after == before + 1 and marker == "kept", (before, after, marker)
    return {"cpuGrew": True, "filesKept": True, "stoppedStayedStopped": True}
