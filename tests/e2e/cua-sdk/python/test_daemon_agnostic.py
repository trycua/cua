"""daemon-agnostic (plan §1.1): sandboxes whose images ship NO cua daemon.

Lifecycle, ports/forward and user-declared readiness probes must work, and
``sb.spacesd()`` must raise ``SpacesdNotAvailable`` (never hang, never a
generic error).
"""

from __future__ import annotations

import asyncio

import e2e
import pytest

import cua


async def _plain_local(
    c: cua.Cua, image: str, port: int, banner: bytes, what: str, memory_mb: int
) -> None:
    sbx = c.sandboxes()
    name = e2e.name(what)
    sb = None
    try:
        sb = await sbx.create(
            cua.SandboxCreateOptions(
                on="local",
                image=image,
                name=name,
                cpus=1,
                memory_mb=memory_mb,
                ports=[port],
                services={"plain": port},
                wait_for=[cua.ReadinessProbe(port=port)],
                ready_timeout_ms=300_000,
            )
        )
        assert sb.location() == "local"
        info = await sb.refresh()
        assert info.status == cua.SandboxStatus.RUNNING, info
        assert name in [s.name for s in await sbx.list("local")]
        assert (await sbx.get(name)).name == name

        # Ports: a forward reaches the guest's own daemon (sshd / Xvnc).
        fwd = await sb.forward(port)
        try:
            # Bounded retry: a daemon may listen a moment before it sends
            # its banner.
            got = await e2e.poll(
                f"{banner!r} banner via forward",
                lambda: asyncio.to_thread(lambda: _banner(fwd.local_addr(), banner)),
                attempts=60,
                delay=1,
            )
            assert got.startswith(banner), got
        finally:
            await fwd.close()

        # Readiness probes: a TCP probe on the open port passes; a probe on a
        # closed port times out with a typed error.
        await sb.wait_ready([cua.ReadinessProbe(port=port)], 30_000)
        with pytest.raises(
            (cua.CuaError.Timeout, cua.CuaError.InvalidArgument, cua.CuaError.NotFound)
        ):
            await sb.wait_ready([cua.ReadinessProbe(port=3999, http_path="/")], 3_000)

        # No spacesd in this image.
        with pytest.raises(cua.CuaError.SpacesdNotAvailable):
            await sb.spacesd(3_000)
    finally:
        if sb is not None:
            await sb.delete()
    assert name not in [s.name for s in await sbx.list("local")]


@pytest.mark.e2e("daemon-agnostic", "hermetic")
def test_agnostic_fake_fleet_and_direct(fixtures, fake_fleet):
    async def body():
        pool = e2e.name("agnostic")
        fleet = fake_fleet.fleet()
        await fleet.apply_pool(
            cua.FleetPoolSpec(name=pool, image="img:plain", services={"server": 8000})
        )
        sb = None
        try:
            sb = await fake_fleet.sandboxes().create(
                cua.SandboxCreateOptions(on="cloud", pool=pool, name=f"{pool}-c")
            )
            # NOTE: sb.services() reports the create options' default
            # {"env": 3211}, not the pool's services (SDK bug, see report);
            # env() correctly consults the bound sandbox's services.
            resp = await sb.service("server").request("GET", "/status", None, 5_000)
            assert resp.status == 200
            with pytest.raises(cua.CuaError.SpacesdNotAvailable):
                await sb.spacesd(2_000)
        finally:
            if sb is not None:
                await sb.delete()
            await fleet.delete_pool(pool)

        # Direct URL where no spacesd answers (the fake Fleet API).
        d = await fake_fleet.sandboxes().connect_url(
            fixtures["fleet_base_url"], "t", e2e.name("nodriver")
        )
        with pytest.raises(cua.CuaError.SpacesdNotAvailable):
            await d.spacesd(2_000)
        await d.delete()

    e2e.run_async(body(), timeout=120)


@pytest.mark.e2e("daemon-agnostic", "container")
def test_agnostic_container_ssh_only(local_cua):
    image = e2e.plain_image("ubuntu-server")
    e2e.require_image(image)
    e2e.run_async(
        _plain_local(local_cua, f"container:{image}", 22, b"SSH-2.0", "plain-ssh", 512), timeout=600
    )


@pytest.mark.e2e("daemon-agnostic", "container")
def test_agnostic_container_vnc_only(local_cua):
    image = e2e.plain_image("ubuntu-xfce-vnc")
    e2e.require_image(image)
    e2e.run_async(
        _plain_local(local_cua, f"container:{image}", 5901, b"RFB 003", "plain-vnc", 1024),
        timeout=600,
    )


@pytest.mark.e2e("daemon-agnostic", "qemu")
def test_agnostic_qemu_ssh_only(local_cua):
    disk = e2e.disk_path("ubuntu-server")
    if not disk.exists():
        pytest.skip(
            f"missing {disk}; build it with libs/images/build.sh plain/ubuntu-server --outputs rootfs,disk"
        )
    e2e.run_async(
        _plain_local(local_cua, f"vm:{disk}", 22, b"SSH-2.0", "qemu-ssh", 1024), timeout=900
    )


@pytest.mark.e2e("daemon-agnostic", "fleet")
def test_agnostic_fleet_legacy_image(live_fleet):
    """The public legacy image has computer-server, not spacesd."""

    async def body():
        pool = e2e.name("agnostic")
        fleet = live_fleet.fleet()
        sb = None
        try:
            await fleet.apply_pool(
                cua.FleetPoolSpec(
                    name=pool,
                    image=e2e.LEGACY_FLEET_ROOTFS,
                    runtime="gvisor",
                    services={"server": 8000},
                    cpu=1,
                    memory_mb=2048,
                    ttl_seconds_after_created=3600,
                )
            )
            sb = await live_fleet.sandboxes().create(
                cua.SandboxCreateOptions(
                    on="cloud",
                    pool=pool,
                    name=f"{pool}-c",
                    ready_timeout_ms=900_000,
                )
            )
            resp = await e2e.poll(
                "server /status",
                lambda: _ok(sb.service("server").request("GET", "/status", None, 30_000)),
                attempts=60,
                delay=5,
            )
            assert resp.status == 200
            with pytest.raises(cua.CuaError.SpacesdNotAvailable):
                await sb.spacesd(10_000)
        finally:
            if sb is not None:
                await sb.delete()
            try:
                await fleet.delete_pool(pool)
            except cua.CuaError.NotFound:
                pass

    e2e.run_async(body(), timeout=1500)


def _banner(addr: str, banner: bytes):
    try:
        got = e2e.read_banner(addr, len(banner), timeout=5)
    except OSError:
        return None
    return got if got.startswith(banner) else None


@pytest.mark.e2e("daemon-agnostic", "container")
def test_tcp_probe_implies_listening(local_cua):
    image = e2e.plain_image("ubuntu-server")
    e2e.require_image(image)

    async def body():
        sb = await local_cua.sandboxes().create(
            cua.SandboxCreateOptions(
                on="local",
                image=f"container:{image}",
                name=e2e.name("probe"),
                cpus=1,
                memory_mb=512,
                ports=[22],
                wait_for=[cua.ReadinessProbe(port=22)],
                ready_timeout_ms=120_000,
            )
        )
        try:
            fwd = await sb.forward(22)
            try:
                got = await asyncio.to_thread(e2e.read_banner, fwd.local_addr(), 7, 5)
            finally:
                await fwd.close()
            assert got.startswith(b"SSH-2.0"), f"ready, but the guest sent {got!r}"
        finally:
            await sb.delete()

    e2e.run_async(body(), timeout=300)


async def _ok(fut):
    r = await fut
    return r if r.status == 200 else None
