"""Snapshots, volumes, secrets, warm pools and the Image builder (hermetic)."""

import os

from harness import case, err_obs


def svc(c, name):
    return getattr(c.daytona, name)


@case(
    "snapshot.create_list_get_delete", needs="mock", ref="_sync/snapshot.py create/list/get/delete"
)
async def snapshot_crud(c):
    sdk = c.sdk
    name = c.name("snap")
    made = await c.w(
        c.daytona.snapshot.create(sdk.CreateSnapshotParams(name=name, image="python:3.12"))
    )
    assert made.name == name and str(made.state).lower().endswith("active"), made.state
    got = await c.w(c.daytona.snapshot.get(name))
    assert got.id == made.id and got.image_name == "python:3.12", got
    page = await c.w(c.daytona.snapshot.list(page=1, limit=100))
    assert name in [s.name for s in page.items] and page.total >= 1 and page.page == 1, page
    sb = await c.new(sdk.CreateSandboxFromSnapshotParams(snapshot=name))
    assert str(sb.state).lower().endswith("started") and sb.snapshot == name, (
        sb.state,
        sb.snapshot,
    )
    await c.w(c.daytona.snapshot.delete(name))
    e = await c.raises(lambda: c.daytona.snapshot.get(name))
    assert e and e["type"] == "DaytonaNotFoundError", e
    return {
        "state": "active",
        "imageName": "python:3.12",
        "sandboxSnapshot": name == sb.snapshot,
        "afterDelete": err_obs(e),
    }


@case("snapshot.pagination", needs="mock", ref="_sync/snapshot.py list(page, limit)")
async def snapshot_pagination(c):
    sdk = c.sdk
    tag = c.name("pg")
    names = []
    for i in range(3):
        n = f"{tag}-{i}"
        await c.w(c.daytona.snapshot.create(sdk.CreateSnapshotParams(name=n, image="python:3.12")))
        names.append(n)
    try:
        p1 = await c.w(c.daytona.snapshot.list(page=1, limit=1))
        assert len(p1.items) == 1 and p1.total >= 3 and p1.total_pages >= 3, (
            len(p1.items),
            p1.total,
            p1.total_pages,
        )
        bad = await c.raises(lambda: c.daytona.snapshot.list(page=0))
        assert bad is not None
        return {"pageSize": len(p1.items), "totalAtLeast3": p1.total >= 3, "pageZeroRefused": True}
    finally:
        for n in names:
            await c.raises(lambda n=n: c.daytona.snapshot.delete(n))


@case("snapshot.duplicate_and_missing", needs="mock", ref="common/errors.py 409/404")
async def snapshot_duplicate_and_missing(c):
    sdk = c.sdk
    name = c.name("dup")
    await c.w(c.daytona.snapshot.create(sdk.CreateSnapshotParams(name=name, image="python:3.12")))
    try:
        dup = await c.raises(
            lambda: c.daytona.snapshot.create(
                sdk.CreateSnapshotParams(name=name, image="python:3.12")
            )
        )
        assert dup and dup["type"] == "DaytonaConflictError" and dup["status_code"] == 409, dup
        miss = await c.raises(lambda: c.daytona.snapshot.get("no-such-snapshot-" + c.run_id))
        assert miss and miss["type"] == "DaytonaNotFoundError", miss
        missing_sb = await c.raises(
            lambda: c.daytona.create(
                sdk.CreateSandboxFromSnapshotParams(snapshot="nope-" + c.run_id)
            )
        )
        assert missing_sb and missing_sb["status_code"] == 404, missing_sb
        return {
            "duplicate": err_obs(dup),
            "missing": err_obs(miss),
            "sandboxFromMissing": err_obs(missing_sb),
        }
    finally:
        await c.raises(lambda: c.daytona.snapshot.delete(name))


@case(
    "snapshot.activate_deactivate",
    needs="mock",
    ref="_sync/snapshot.py activate; snapshots_api deactivate",
)
async def snapshot_activate(c):
    sdk = c.sdk
    name = c.name("act")
    s = await c.w(
        c.daytona.snapshot.create(sdk.CreateSnapshotParams(name=name, image="python:3.12"))
    )
    try:
        already = await c.raises(lambda: c.daytona.snapshot.activate(name))
        # `activate` lets the generated client's exception through (Python), a
        # DaytonaError (TypeScript): the status is what they share.
        assert already and already["status_code"] == 400, already
        return {"activateActive": already["status_code"]}
    finally:
        await c.raises(lambda: c.daytona.snapshot.delete(name))


@case("snapshot.from_sandbox", needs="mock", ref="_sync/sandbox.py create_snapshot")
async def snapshot_from_sandbox(c):
    sdk = c.sdk
    sb = await c.new()
    name = c.name("fromsb")
    await c.w(sb.create_snapshot(name))
    try:
        got = await c.w(c.daytona.snapshot.get(name))
        assert got.name == name
        kid = await c.new(sdk.CreateSandboxFromSnapshotParams(snapshot=name))
        assert str(kid.state).lower().endswith("started")
        dup = await c.raises(lambda: sb.create_snapshot(name))
        assert dup and dup["type"] == "DaytonaConflictError", dup
        return {"registered": True, "sandboxFrom": True, "duplicate": err_obs(dup)}
    finally:
        await c.raises(lambda: c.daytona.snapshot.delete(name))


@case(
    "snapshot.build_from_image_object",
    needs="mock",
    ref="_sync/snapshot.py create (buildInfo, on_logs); common/image.py",
)
async def snapshot_build(c):
    sdk = c.sdk
    name = c.name("built")
    image = (
        sdk.Image.debian_slim("3.12").pip_install(["requests"]).env({"BUILT": "yes"}).workdir("/w")
    )
    logs = []
    s = await c.w(
        c.daytona.snapshot.create(
            sdk.CreateSnapshotParams(name=name, image=image), on_logs=logs.append
        )
    )
    try:
        assert str(s.state).lower().endswith("active"), s.state
        assert s.build_info is not None and "FROM python:3.12" in s.build_info.dockerfile_content
        assert any("Creating snapshot" in lnk for lnk in logs), logs
        assert any("Created snapshot" in lnk for lnk in logs), logs
        sb = await c.new(sdk.CreateSandboxFromSnapshotParams(snapshot=name))
        assert str(sb.state).lower().endswith("started")
        return {"state": "active", "dockerfile": True, "logged": True}
    finally:
        await c.raises(lambda: c.daytona.snapshot.delete(name))


@case(
    "snapshot.build_failure_is_reported",
    needs="mock",
    ref="_sync/snapshot.py create (BUILD_FAILED -> DaytonaError)",
)
async def snapshot_build_failure(c):
    sdk = c.sdk
    name = c.name("bad")
    image = sdk.Image.base("python:3.12").dockerfile_commands(["ONBUILD RUN true"])
    e = await c.raises(
        lambda: c.daytona.snapshot.create(sdk.CreateSnapshotParams(name=name, image=image))
    )
    try:
        assert e and e["type"] == "DaytonaError" and "Failed to create snapshot" in e["message"], e
        return {"type": e["type"], "failed": True}
    finally:
        await c.raises(lambda: c.daytona.snapshot.delete(name))


# ── volumes ────────────────────────────────────────────────────────────────


@case("volume.crud", needs="mock", ref="_sync/volume.py")
async def volume_crud(c):
    name = c.name("vol")
    v = await c.w(c.daytona.volume.create(name))
    try:
        assert v.name == name and str(v.state).lower().endswith("ready"), v.state
        assert (await c.w(c.daytona.volume.get(name))).id == v.id
        assert name in [x.name for x in await c.w(c.daytona.volume.list())]
        again = await c.w(c.daytona.volume.get(name, create=True))
        assert again.id == v.id
        dup = await c.raises(lambda: c.daytona.volume.create(name))
        assert dup and dup["type"] == "DaytonaConflictError", dup
        bad = await c.raises(lambda: c.daytona.volume.create("bad name!"))
        assert bad and bad["status_code"] == 400, bad
        fresh = await c.w(c.daytona.volume.get(name + "-new", create=True))
        await c.w(c.daytona.volume.delete(fresh))
        gone = await c.raises(lambda: c.daytona.volume.get(name + "-new"))
        assert gone and gone["type"] == "DaytonaNotFoundError", gone
        return {
            "state": "ready",
            "duplicate": err_obs(dup),
            "invalidName": err_obs(bad),
            "afterDelete": err_obs(gone),
        }
    finally:
        await c.raises(lambda: c.daytona.volume.delete(v))


@case(
    "volume.shared_between_sandboxes",
    needs="space",
    ref="common/volume.py VolumeMount; _sync/volume.py",
    requires="CUA_COMPAT_VOLUME_GRANT",
    why="Cua Volume gives a Space write access only to its own folder; sharing one needs a grant the user confirms (cua volume grant)",
)
async def volume_shared(c):
    sdk = c.sdk
    v = await c.w(c.daytona.volume.create(c.name("shr")))
    try:
        mount = sdk.VolumeMount(volume_id=v.id, mount_path="/data")
        a = await c.new(sdk.CreateSandboxFromSnapshotParams(volumes=[mount]))
        w = await c.w(a.process.exec("echo shared-by-a > /data/note.txt && cat /data/note.txt"))
        assert w.exit_code == 0 and w.result.strip() == "shared-by-a", (w.exit_code, w.result)
        b = await c.new(sdk.CreateSandboxFromSnapshotParams(volumes=[mount]))
        r = await c.w(b.process.exec("cat /data/note.txt"))
        assert r.result.strip() == "shared-by-a", r.result
        return {"shared": True}
    finally:
        await c.raises(lambda: c.daytona.volume.delete(v))


@case(
    "volume.mount_without_a_grant_is_explained", needs="space", ref="common/volume.py VolumeMount"
)
async def volume_no_grant(c):
    sdk = c.sdk
    v = await c.w(c.daytona.volume.create(c.name("ng")))
    try:
        e = await c.raises(
            lambda: c.daytona.create(
                sdk.CreateSandboxFromSnapshotParams(
                    volumes=[sdk.VolumeMount(volume_id=v.id, mount_path="/data")]
                )
            )
        )
        if e is None:
            return {"mounted": True}  # a grant is in place on this machine
        assert (
            e["status_code"] == 400
            and "Cua Volume" in e["message"]
            and "cua volume grant" in e["message"]
        ), e
        return {"refused": 400, "explainsGrant": True}
    finally:
        await c.raises(lambda: c.daytona.volume.delete(v))


@case(
    "volume.mount_needs_cua_volume",
    needs="hermetic",
    ref="common/volume.py VolumeMount; models/sandbox_volume.py",
)
async def volume_mount_needs_volume(c):
    sdk = c.sdk
    name = c.name("mnt")
    v = await c.w(c.daytona.volume.create(name))
    try:
        # A mock Space has no Cua Volume mounted: Daytona's create fails with the reason
        # and leaves no sandbox behind.
        e = await c.raises(
            lambda: c.daytona.create(
                sdk.CreateSandboxFromSnapshotParams(
                    labels={"vm": c.run_id},
                    volumes=[sdk.VolumeMount(volume_id=v.id, mount_path="/data")],
                )
            )
        )
        assert e and e["status_code"] == 400 and "Cua Volume" in e["message"], e
        left = await c.list(labels={"vm": c.run_id})
        assert left == [], left
        missing = await c.raises(
            lambda: c.daytona.create(
                sdk.CreateSandboxFromSnapshotParams(
                    volumes=[sdk.VolumeMount(volume_id="no-such-volume", mount_path="/data")]
                )
            )
        )
        assert missing and missing["status_code"] == 404, missing
        bad_path = await c.raises(
            lambda: c.daytona.create(
                sdk.CreateSandboxFromSnapshotParams(
                    volumes=[sdk.VolumeMount(volume_id=v.id, mount_path="relative")]
                )
            )
        )
        assert bad_path and bad_path["status_code"] == 400, bad_path
        return {
            "noCuaVolume": err_obs(e),
            "missingVolume": err_obs(missing),
            "relativePath": err_obs(bad_path),
        }
    finally:
        await c.raises(lambda: c.daytona.volume.delete(v))


# ── secrets ────────────────────────────────────────────────────────────────


@case("secret.crud", needs="mock", ref="_sync/secret.py; common/secret.py")
async def secret_crud(c):
    sdk = c.sdk
    name = c.name("sec").replace("-", "_")
    s = await c.w(
        c.daytona.secret.create(
            sdk.CreateSecretParams(
                name=name, value="s3cret", hosts=["api.example.com"], description="d"
            )
        )
    )
    try:
        assert (
            s.name == name
            and s.hosts == ["api.example.com"]
            and s.placeholder
            and "s3cret" not in s.placeholder
        )
        assert (await c.w(c.daytona.secret.get(s.id))).id == s.id
        page = await c.w(c.daytona.secret.list(limit=100))
        assert name in [x.name for x in page.items] and page.total >= 1, page
        up = await c.w(
            c.daytona.secret.update(
                s.id, sdk.UpdateSecretParams(value="new", hosts=["*.example.com"])
            )
        )
        assert up.hosts == ["*.example.com"] and up.updated_at >= s.updated_at
        # The secret service lets the generated client's exceptions through (Python):
        # the HTTP status is what both SDKs share.
        dup = await c.raises(
            lambda: c.daytona.secret.create(sdk.CreateSecretParams(name=name, value="x"))
        )
        assert dup and dup["status_code"] == 409, dup
        badname = await c.raises(
            lambda: c.daytona.secret.create(sdk.CreateSecretParams(name="1bad", value="x"))
        )
        assert badname and badname["status_code"] == 400, badname
        badhost = await c.raises(
            lambda: c.daytona.secret.create(
                sdk.CreateSecretParams(name=name + "x", value="x", hosts=["a.com:443"])
            )
        )
        assert badhost and badhost["status_code"] == 400, badhost
        await c.w(c.daytona.secret.delete(s.id))
        gone = await c.raises(lambda: c.daytona.secret.get(s.id))
        assert gone and gone["status_code"] == 404, gone
        return {
            "valueNotReturned": True,
            "hosts": ["*.example.com"],
            "duplicate": 409,
            "badName": 400,
            "badHost": 400,
            "afterDelete": 404,
        }
    finally:
        await c.raises(lambda: c.daytona.secret.delete(s.id))


@case(
    "secret.mount_is_opt_in",
    needs="mock",
    ref="models/create_sandbox.py secrets; Cua: no egress substitution proxy",
)
async def secret_mount(c):
    sdk = c.sdk
    name = c.name("mnt").replace("-", "_")
    s = await c.w(c.daytona.secret.create(sdk.CreateSecretParams(name=name, value="v-123")))
    try:
        refused = await c.raises(
            lambda: c.daytona.create(sdk.CreateSandboxFromSnapshotParams(secrets={"MY_KEY": name}))
        )
        assert (
            refused
            and refused["status_code"] == 400
            and "cua.secrets=plaintext" in refused["message"]
        ), refused
        sb = await c.new(
            sdk.CreateSandboxFromSnapshotParams(
                secrets={"MY_KEY": name}, labels={"cua.secrets": "plaintext"}
            )
        )
        assert sb.env.get("MY_KEY") == "v-123", sb.env
        missing = await c.raises(
            lambda: c.daytona.create(
                sdk.CreateSandboxFromSnapshotParams(
                    secrets={"K": "no-such-secret"}, labels={"cua.secrets": "plaintext"}
                )
            )
        )
        assert missing and missing["status_code"] == 404, missing
        return {
            "refusedWithoutLabel": err_obs(refused),
            "injected": True,
            "missing": err_obs(missing),
        }
    finally:
        await c.raises(lambda: c.daytona.secret.delete(s.id))


# ── image builder ──────────────────────────────────────────────────────────


@case(
    "image.create_with_layers",
    needs="mock",
    ref="_sync/daytona.py _create (PENDING_BUILD, on_snapshot_create_logs)",
)
async def image_create_with_layers(c):
    sdk = c.sdk
    image = (
        sdk.Image.debian_slim("3.12")
        .pip_install(["requests"])
        .run_commands("echo built")
        .env({"X": "1"})
    )
    logs = []
    sb = await c.new(
        sdk.CreateSandboxFromImageParams(image=image, labels={"img": c.run_id}),
    )
    assert str(sb.state).lower().endswith("started"), sb.state
    return {"started": True}


@case(
    "image.context_upload",
    needs="mock",
    ref="_sync/object_storage.py upload; common/image.py add_local_file/add_local_dir",
    requires="CUA_COMPAT_OS_TRUSTS_CA",
    why="obstore (the Python SDK's S3 client) ignores SSL_CERT_FILE and needs the gateway CA in the OS trust store",
)
async def image_context_upload(c):
    import tempfile

    sdk = c.sdk
    with tempfile.TemporaryDirectory() as d:
        os.makedirs(os.path.join(d, "pkg", "sub"))
        open(os.path.join(d, "pkg", "a.txt"), "w").write("a")
        open(os.path.join(d, "pkg", "sub", "b.txt"), "w").write("b")
        open(os.path.join(d, "single.txt"), "w").write("single")
        image = (
            sdk.Image.debian_slim("3.12")
            .add_local_file(os.path.join(d, "single.txt"), "/home/daytona/single.txt")
            .add_local_dir(os.path.join(d, "pkg"), "/home/daytona/pkg")
        )
        sb = await c.new(sdk.CreateSandboxFromImageParams(image=image))
        assert str(sb.state).lower().endswith("started"), sb.state
        # a second create of the same context finds it already uploaded
        sb2 = await c.new(sdk.CreateSandboxFromImageParams(image=image))
        assert str(sb2.state).lower().endswith("started")
        return {"uploadedAndReused": True}


@case("image.unbuildable_dockerfile_is_refused", needs="mock", ref="Cua: Dockerfile subset")
async def image_unbuildable(c):
    sdk = c.sdk
    image = sdk.Image.base("python:3.12").dockerfile_commands(["ONBUILD RUN true"])
    e = await c.raises(lambda: c.daytona.create(sdk.CreateSandboxFromImageParams(image=image)))
    assert e and e["status_code"] == 400 and "ONBUILD" in e["message"], e
    return {"refused": err_obs(e)}


# ── warm pools ─────────────────────────────────────────────────────────────


@case("warm_pool.crud", needs="mock", ref="_sync/warm_pool.py")
async def warm_pool_crud(c):
    sdk = c.sdk
    snap = c.name("wp")
    await c.w(c.daytona.snapshot.create(sdk.CreateSnapshotParams(name=snap, image="python:3.12")))
    try:
        p = await c.w(c.daytona.warm_pool.create(snap, pool=2))
        try:
            assert p.snapshot == snap and p.pool == 2, p
            assert p.id in [x.id for x in await c.w(c.daytona.warm_pool.list())]
            dup = await c.raises(lambda: c.daytona.warm_pool.create(snap, pool=1))
            assert dup and dup["status_code"] == 409, dup
            up = await c.w(c.daytona.warm_pool.update(p.id, pool=3))
            assert up.pool == 3
            return {"pool": 3, "duplicate": 409}
        finally:
            await c.w(c.daytona.warm_pool.delete(p.id))
    finally:
        await c.raises(lambda: c.daytona.snapshot.delete(snap))
