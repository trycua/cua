# docs: test="container"
import asyncio

from cua_sandbox import Image, Sandbox, http


async def main():
    sb = await Sandbox.create(
        Image.from_registry("python:3.12-slim"),
        command=["python", "-m", "http.server", "8000"],
        services={"web": 8000},
        wait_for=http("web", "/"),
        cpu=2, memory="4GB",
    )
    print(sb.id)
    await sb.destroy()


asyncio.run(main())
