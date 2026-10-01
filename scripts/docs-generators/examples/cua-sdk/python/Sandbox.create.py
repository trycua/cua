# docs: test="docs" prelude="fakefleet"
import asyncio

from cua_sandbox import CloudOptions, Image, Sandbox, http


async def main():
    sb = await Sandbox.create(
        Image.from_registry("python:3.12-slim"),
        command=["python", "-m", "http.server", "8000"],
        services={"web": 8000},
        wait_for=http("web", "/"),
        local=False,                     # the cloud; omit it to run locally
        cloud=CloudOptions(warm=False),  # optional; implies the cloud
        cpu=2, memory="4GB",
    )
    print(sb.id)
    await sb.destroy()


asyncio.run(main())
