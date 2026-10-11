# docs: test="docs" prelude="fakefleet"
import asyncio

from cua_sandbox import CloudOptions, Image, Pool, PoolOptions, Sandbox, SandboxSpec


async def main():
    pool = await Pool.apply("my-team-linux", SandboxSpec(image=Image.linux()), PoolOptions(replicas=2))
    sb = await Sandbox.create(cloud=CloudOptions(pool=pool))  # or CloudOptions(pool="my-team-linux")
    print(sb.id)
    await sb.destroy()
    await pool.delete()


asyncio.run(main())
