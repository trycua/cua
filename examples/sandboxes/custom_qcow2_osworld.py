"""Custom qcow2.zip VM via bare-metal QEMU (OSWorld example).

Downloads the OSWorld Ubuntu qcow2.zip from HuggingFace, extracts it,
and boots it with QEMU. Uses the OSWorld Flask transport (port 5000)
for screenshots and commands.

    uv run python examples/sandboxes/custom_qcow2_osworld.py
"""

import asyncio

from cua_sandbox import Image, sandbox
from cua_sandbox.runtime import QEMURuntime

OSWORLD_IMAGE = (
    "https://huggingface.co/datasets/xlangai/ubuntu_osworld/resolve/main/Ubuntu.qcow2.zip"
)


async def main():
    # Bare-metal QEMU with VNC for live viewing
    runtime = QEMURuntime(
        mode="bare-metal",
        api_port=18020,
        vnc_display=20,
        memory_mb=4096,
        cpu_count=4,
    )

    # from_file downloads the zip, extracts the qcow2, and caches it locally
    # agent_type="osworld" tells the sandbox to use the OSWorld Flask transport
    image = Image.from_file(
        OSWORLD_IMAGE,
        os_type="linux",
        agent_type="osworld",
    )

    async with sandbox(
        local=True,
        image=image,
        runtime=runtime,
        name="osworld-demo",
    ) as sb:
        screenshot = await sb.screenshot()
        with open("/tmp/osworld-screenshot.png", "wb") as f:
            f.write(screenshot)
        print(f"Screenshot saved ({len(screenshot)} bytes)")

        w, h = await sb.get_dimensions()
        print(f"Screen: {w}x{h}")

        print("Press Ctrl+C to stop.")
        try:
            while True:
                await asyncio.sleep(1)
        except KeyboardInterrupt:
            print("Shutting down...")


if __name__ == "__main__":
    asyncio.run(main())
