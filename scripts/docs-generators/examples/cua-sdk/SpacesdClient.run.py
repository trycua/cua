# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

import cua


async def main():
    guest = await cua.embedded().spacesd(URL, TOKEN)  # or: await sandbox.spacesd(None)
    out = await guest.run(cua.SpacesdCommand(program="echo", args=["hello"]))
    print([out.exit.success, out.stdout])
    # [True, b'hello\n']


asyncio.run(main())
