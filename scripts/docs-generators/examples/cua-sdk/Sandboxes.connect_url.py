# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

import cua


async def main():
    sb = await cua.embedded().sandboxes().connect_url(URL, TOKEN, "dev")  # your spacesd's address and token
    guest = await sb.spacesd(None)
    out = await guest.run(cua.SpacesdCommand(program="echo", args=["hi"]))
    print(out.stdout)


asyncio.run(main())
