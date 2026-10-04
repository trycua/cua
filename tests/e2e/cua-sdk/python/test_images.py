"""images + image-build-push-run (the replacement for the minecraft guide's
hand-made containerDisk).

* image-build-push-run: ``cua image build`` (the CLI) applies layers on a
  base, pushes to a registry, and the pushed reference boots as a sandbox
  (container lane: rootfs; qemu lane: containerDisk on the reference disk).
* images: the same through the SDK (``Cua.local().build_image``).

The registry is a throwaway ``registry:2`` container on loopback
(``cua-e2e-<run>-registry-py``), pushed to as an insecure registry.
"""

from __future__ import annotations

import contextlib
import json
import os
import secrets
import subprocess
from pathlib import Path

import e2e
import pytest

import cua


def spec(name: str, marker: str) -> dict:
    return {
        "apiVersion": "images.cua.ai/v1alpha1",
        "kind": "Image",
        "metadata": {"name": name, "namespace": name},
        "spec": {
            "recipe": {
                "osType": "linux",
                "distro": "ubuntu",
                "version": "24.04",
                "kind": "vm",
                "layers": [
                    {
                        "type": "run",
                        "command": f"mkdir -p /opt/cua-e2e && echo {marker} > /opt/cua-e2e/marker",
                    }
                ],
                "env": {"CUA_E2E_BUILT": marker},
                "ports": [8080],
            }
        },
    }


@contextlib.contextmanager
def registry(tag: str):
    name = e2e.name(f"registry-{tag}")
    e2e.docker("rm", "-f", name, check=False)
    try:
        e2e.docker(
            "run",
            "-d",
            "--name",
            name,
            "--memory=256m",
            "-p",
            "127.0.0.1::5000",
            "registry:2",
            timeout=300,
        )
        port = e2e.docker("port", name, "5000/tcp").stdout.strip().splitlines()[0].rsplit(":", 1)[1]
        host = f"127.0.0.1:{port}"

        async def up():
            status, _ = await __import__("asyncio").to_thread(e2e.http_get, f"http://{host}/v2/")
            return status == 200

        e2e.run_async(e2e.poll("registry", up, attempts=30, delay=1), timeout=60)
        old = os.environ.get("CUA_INSECURE_REGISTRIES")
        os.environ["CUA_INSECURE_REGISTRIES"] = host
        try:
            yield host
        finally:
            if old is None:
                os.environ.pop("CUA_INSECURE_REGISTRIES", None)
            else:
                os.environ["CUA_INSECURE_REGISTRIES"] = old
    finally:
        e2e.docker("rm", "-f", name, check=False)


def _run_built_container(c: cua.Cua, ref: str, marker: str) -> None:
    """Boot the pushed rootfs as a local sandbox; the layers' effects are there."""

    async def body():
        name = e2e.name("built")
        sb = await c.sandboxes().create(
            cua.SandboxCreateOptions(
                on="local",
                image=f"container:{ref}",
                name=name,
                cpus=1,
                memory_mb=512,
                wait_for=[],
                ready_timeout_ms=300_000,
            )
        )
        try:
            # The base (plain ubuntu-server) has no spacesd: the oracle is
            # the engine itself, not the SDK.
            out = e2e.docker(
                "exec",
                name,
                "sh",
                "-c",
                "cat /opt/cua-e2e/marker; . /etc/profile.d/cua-env.sh; echo $CUA_E2E_BUILT",
            ).stdout.split()
            assert out == [marker, marker], out
            with pytest.raises(cua.CuaError.SpacesdNotAvailable):
                await sb.spacesd(3_000)
        finally:
            await sb.delete()

    e2e.run_async(body(), timeout=600)


@pytest.mark.e2e("image-build-push-run", "container")
def test_cli_build_push_run_container(local_cua, tmp_path):
    cli = e2e.cua_cli()
    if cli is None:
        pytest.skip("the cua CLI is not built")
    base = e2e.plain_image("ubuntu-server")
    e2e.require_image(base)
    marker = secrets.token_hex(6)
    spec_path = tmp_path / "image.json"
    spec_path.write_text(json.dumps(spec(e2e.name("img"), marker)))
    with registry("cli") as host:
        dest = f"{host}/cua-e2e/built:{e2e.RUN}-py"
        out = subprocess.run(
            [
                str(cli),
                "--embedded",
                "--json",
                "image",
                "build",
                str(spec_path),
                "--base",
                f"container:{base}",
                "--push",
                dest,
            ],
            capture_output=True,
            text=True,
            timeout=1200,
            env=dict(os.environ),
        )
        assert out.returncode == 0, out.stderr[-2000:]
        pushed = out.stdout.strip().splitlines()[-1]
        print("cua image build ->", pushed)
        assert "@sha256:" in pushed, pushed
        _run_built_container(local_cua, dest, marker)


@pytest.mark.e2e("images", "container")
def test_sdk_local_layers_apply(local_cua):
    base = e2e.plain_image("ubuntu-server")
    e2e.require_image(base)
    marker = secrets.token_hex(6)
    with registry("sdk") as host:
        dest = f"{host}/cua-e2e/sdk-built:{e2e.RUN}-py"
        ref = e2e.run_async(
            local_cua.local().build_image(
                json.dumps(spec(e2e.name("img-sdk"), marker)), f"container:{base}", dest
            ),
            timeout=1200,
        )
        assert ref.startswith(dest) and "@sha256:" in ref, ref
        _run_built_container(local_cua, dest, marker)


@pytest.mark.e2e("image-build-push-run", "qemu")
def test_cli_build_push_boot_vm(local_cua, tmp_path):
    """VM build on the reference disk -> containerDisk -> push -> boot the
    pushed reference under QEMU (the minecraft guide's flow, automated)."""
    cli = e2e.cua_cli()
    disk = Path(
        # CUA_E2E_DISK_CUA_DESKTOP_LINUX: the pre-rename name, read for one release.
        os.environ.get("CUA_E2E_DISK_LINUX")
        or os.environ.get(
            "CUA_E2E_DISK_CUA_DESKTOP_LINUX",
            str(Path.home() / ".cache" / "cua-images-e2e" / "linux" / e2e.HOST_ARCH / "disk.img"),
        )
    )
    if cli is None or not disk.exists():
        pytest.skip("needs the cua CLI and the reference disk")
    marker = secrets.token_hex(6)
    s = spec(e2e.name("img-vm"), marker)
    s["spec"]["recipe"]["kind"] = "vm"
    spec_path = tmp_path / "image.json"
    spec_path.write_text(json.dumps(s))
    with registry("vm") as host:
        dest = f"{host}/cua-e2e/built-vm:{e2e.RUN}-py"
        out = subprocess.run(
            [
                str(cli),
                "--embedded",
                "--json",
                "image",
                "build",
                str(spec_path),
                "--base",
                f"vm:{disk}",
                "--push",
                dest,
            ],
            capture_output=True,
            text=True,
            timeout=2400,
            env=dict(os.environ),
        )
        assert out.returncode == 0, out.stderr[-2000:]

        # A fresh runtime: the QEMU disk resolver reads CUA_INSECURE_REGISTRIES
        # when the runtime is created, not per pull.
        fresh = cua.embedded(state_dir=str(tmp_path / "boot-state"), fleet_from_env=False)

        async def boot():
            sb = await fresh.sandboxes().create(
                cua.SandboxCreateOptions(
                    on="local",
                    image=f"vm:{dest}",
                    name=e2e.name("built-vm"),
                    cpus=2,
                    memory_mb=3072,
                    ports=[3211],
                    wait_for=[cua.ReadinessProbe(port=3211, http_path="/viewer/")],
                    ready_timeout_ms=600_000,
                )
            )
            try:
                assert sb.runtime_type() == "qemu"
            finally:
                await sb.delete()

        e2e.run_async(boot(), timeout=1200)
