# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

from cua_sandbox import Sandbox


async def main():
    async with Sandbox.connect(url=URL, token=TOKEN) as sb:  # or Sandbox.connect("my-sandbox")
        result = await sb.shell.run("echo hello")
        print([result.stdout, result.returncode, result.success])


asyncio.run(main())
