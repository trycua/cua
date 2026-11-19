# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

import cua


async def main():
    guest = await cua.embedded().spacesd(URL, TOKEN)
    shot = await guest.screenshot(None)  # PNG by default
    await guest.click(shot.width / 2, shot.height / 2)
    print([shot.width > 0, shot.image[:4]])
    # [True, b'\x89PNG']


asyncio.run(main())
