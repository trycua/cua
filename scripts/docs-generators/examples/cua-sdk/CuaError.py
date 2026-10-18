# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

import cua


async def main():
    sb = await cua.embedded().sandboxes().connect_url(URL, "wrong-token", None)
    try:
        await sb.spacesd(5000)
    except cua.CuaError.Unauthenticated as e:  # one variant
        print("rejected:", e)
    except cua.CuaError as e:  # any other
        raise


asyncio.run(main())
