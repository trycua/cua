# docs: test="docs" prelude="space-url"
import asyncio

import cua


async def main():
    spaces = cua.embedded().spaces()
    info = await spaces.add("http://10.0.0.5:3211", "TOKEN", "lab")  # a machine running cua-spacesd
    space = await spaces.space(info.id)
    out = await space.bash("echo hi", None)
    print([info.provider, out.stdout, out.exit_code])


asyncio.run(main())
