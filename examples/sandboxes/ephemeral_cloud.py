"""Ephemeral cloud Linux VM via the CUA platform API.

Spins up a cloud VM, takes a screenshot, runs a command, then destroys
the VM on exit.

    CUA_API_KEY=sk-... uv run python examples/sandboxes/ephemeral_cloud.py
"""

import asyncio

from cua_sandbox import Image, Sandbox


async def main():
    async with Sandbox.ephemeral(Image.linux()) as sb:
        screenshot = await sb.screenshot()
        with open("/tmp/ephemeral-cloud-screenshot.png", "wb") as f:
            f.write(screenshot)
        print(f"Screenshot saved ({len(screenshot)} bytes)")

        result = await sb.shell.run("uname -a")
        print(f"uname: {result.stdout.strip()}")

        result = await sb.shell.run("echo hello from the cloud")
        print(f"echo: {result.stdout.strip()}")


if __name__ == "__main__":
    asyncio.run(main())
