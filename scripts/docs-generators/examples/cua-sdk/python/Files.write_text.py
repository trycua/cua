# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

from cua_sandbox import Sandbox


async def main():
    async with Sandbox.connect(url=URL, token=TOKEN) as sb:
        await sb.files.write_text("/tmp/note.txt", "hi")
        print([await sb.files.read_text("/tmp/note.txt")])
        # ['hi']


asyncio.run(main())
