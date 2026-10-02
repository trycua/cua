# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

import cua


async def main():
    guest = await cua.embedded().spacesd(URL, TOKEN)
    proc = await guest.spawn(cua.SpacesdCommand(program="cat", args=[], stdin=True))
    await proc.write_stdin(b"xyz")
    await proc.close_stdin()
    print([(await proc.wait()).stdout])
    # [b'xyz']


asyncio.run(main())
