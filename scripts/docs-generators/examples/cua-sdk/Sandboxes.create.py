# docs: test="container"
import asyncio

import cua


async def main():
    c = cua.embedded()
    sb = await c.sandboxes().create(cua.SandboxCreateOptions(
        on="local",                                    # or "cloud"
        image=cua.canonical_image("linux", None),
        name="dev",
        wait_for=[cua.ReadinessProbe(service="env")],  # until its spacesd answers
    ))
    guest = await sb.spacesd(None)
    print((await guest.sh("uname -a", None)).stdout.decode())
    await sb.delete()


asyncio.run(main())
