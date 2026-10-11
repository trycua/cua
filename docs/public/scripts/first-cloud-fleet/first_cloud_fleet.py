# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = [
#   "cua-sandbox==0.9.0",
# ]
# [[tool.uv.index]]
# name = "cua-wheels"
# url = "https://wheels.cua.ai/simple"
# ///

import asyncio
import hashlib
import os
from pathlib import Path

from cua_sandbox import Image, Sandbox

SOURCE_TEXT = "pear\napple\npear\nbanana\n"
SOURCE_PATH = "/tmp/first-cloud-fleet.txt"
SCREENSHOT = Path("first-cloud-fleet.png")


async def run_tutorial(image_ref: str | None = None) -> None:
    # Unset: the canonical Linux desktop, ghcr.io/trycua/linux:24.04.
    image = Image.from_registry(image_ref) if image_ref else Image.linux()
    # local=False runs it in the cloud. A managed pool (cua-auto-*) is created
    # for this image on first use and reused afterwards. Leaving the block
    # releases the claim.
    async with Sandbox.ephemeral(image, local=False, cpu=4, memory_mb=4096) as sandbox:
        print(f"Claimed {sandbox.claim_name} from pool {sandbox.pool_name}")

        await sandbox.files.write_text(SOURCE_PATH, SOURCE_TEXT)
        result = await sandbox.shell.run(f"sha256sum {SOURCE_PATH}")
        if not result.success:
            raise RuntimeError(result.stderr)

        guest_digest = result.stdout.split()[0]
        local_digest = hashlib.sha256(SOURCE_TEXT.encode()).hexdigest()
        if guest_digest != local_digest:
            raise RuntimeError(f"Verification failed: guest={guest_digest} local={local_digest}")
        print(f"Verified SHA-256: {guest_digest}")

        SCREENSHOT.write_bytes(await sandbox.screenshot())
        print(f"Screenshot: {SCREENSHOT.resolve()}")
    print("Claim released")


if __name__ == "__main__":
    asyncio.run(run_tutorial(os.environ.get("CUA_FLEET_IMAGE")))
