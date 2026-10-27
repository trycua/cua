"""Templates and volumes on real Spaces (opt-in; boots a VM).

    CUA_HOME=... CUA_E2B_ON=local python smoke_templates.py

Prints a JSON report: a volume round trip on Cua Volume, a VM template built
from a recipe (its started service already answers in a sandbox created from
it), the refusal of a bare OCI base, and the
volume_mounts answer for a VM.
"""

from __future__ import annotations

import asyncio
import json
import time
import urllib.request

import e2b
from e2b import AsyncSandbox, AsyncTemplate, AsyncVolume

report: dict = {}


async def volumes():
    name = f"smoke-{int(time.time())}"
    v = await AsyncVolume.create(name)
    await v.write_file("/a/b.txt", "hello", mode=0o600, force=True)
    report["volume"] = {
        "listed": name in [i.name for i in await AsyncVolume.list()],
        "read": await v.read_file("/a/b.txt"),
        "exists": [await v.exists("/a"), await v.exists("/a/b.txt"), await v.exists("/zz")],
        "mode": (await v.update_metadata("/a/b.txt", uid=5)).uid,
    }
    await v.remove("/a")
    report["volume"]["after_remove"] = await v.exists("/a/b.txt")
    report["volume"]["destroyed"] = await AsyncVolume.destroy(name)
    report["volume"]["destroyed_again"] = await AsyncVolume.destroy(name)
    return name


async def vm_template():
    t0 = time.time()
    logs = []
    t = (
        e2b.Template()
        .from_base_image()
        .set_envs({"WORLD": "smoke"})
        .run_cmd('mkdir -p /tmp/www && echo "world $WORLD" > /tmp/www/index.html')
        .set_start_cmd("cd /tmp/www && python3 -m http.server 8099", e2b.wait_for_port(8099))
    )
    info = await AsyncTemplate.build(t, "smoke-tpl", on_build_logs=logs.append)
    report["vm_template"] = {"build_s": round(time.time() - t0, 1), "name": info.name}
    t1 = time.time()
    sb = await AsyncSandbox.create(template="smoke-tpl", timeout=300)
    try:
        report["vm_template"]["create_s"] = round(time.time() - t1, 2)
        url = f"http://{sb.get_host(8099)}/"
        report["vm_template"]["served"] = urllib.request.urlopen(url, timeout=20).read().decode()
        report["vm_template"]["env"] = (
            await sb.commands.run("printf %s $WORLD", user="cua")
        ).stdout
        try:
            await AsyncSandbox.create(template="smoke-tpl", volume_mounts={"/mnt/x": "nope"})
        except Exception as e:  # noqa: BLE001
            report["vm_template"]["volume_mounts"] = f"{type(e).__name__}: {e}"[:200]
    finally:
        await sb.kill()
    report["vm_template"]["deleted"] = await AsyncTemplate.delete("smoke-tpl")
    report["vm_template"]["snapshots_left"] = [
        i.snapshot_id for i in await AsyncSandbox.list_snapshots().next_items()
    ]


async def oci_template():
    try:
        await AsyncTemplate.build(e2b.Template().from_image("alpine:3.20"), "smoke-oci")
        report["oci_template"] = "built"
    except e2b.BuildException as e:
        report["oci_template"] = f"BuildException: {e}"[:300]


async def main():
    try:
        await volumes()
    except e2b.VolumeException as e:  # a plain daemon: Cua Volume ships with Cua Spaces
        report["volume"] = f"skipped: {e}"[:200]
    await vm_template()
    await oci_template()
    print(json.dumps(report, indent=1))


asyncio.run(main())
