# docs: test="docs" prelude="spacesd,spacesd-vars"
import asyncio

from cua_sandbox import Sandbox


async def main():
    async with Sandbox.connect(url=URL, token=TOKEN) as sb:
        width, height = await sb.screen.size()
        await sb.mouse.click(width // 2, height // 2)
        await sb.keyboard.type("hello")
        await sb.keyboard.keypress(["ctrl", "a"])
        png = await sb.screenshot()
        print(png[:4])
        # b'\x89PNG'


asyncio.run(main())
