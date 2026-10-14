# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

import cua


async def main():
    guest = await cua.embedded().spacesd(URL, TOKEN)
    sent = await guest.upload("/tmp/note.txt", b"hi from the host", None)
    back = await guest.download("/tmp/note.txt")
    print([sent.size, back])
    # [16, b'hi from the host']


asyncio.run(main())
