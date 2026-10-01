# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = [
#   "cua-sandbox==0.7.0",
# ]
# [[tool.uv.index]]
# name = "cua-wheels"
# url = "https://wheels.cua.ai/simple"
# ///

import asyncio
import hashlib
import os
from uuid import uuid4

from cua_sandbox import Image, Pool


SOURCE_TEXT = "pear\napple\npear\nbanana\n"
SOURCE_PATH = "/tmp/first-cloud-fleet.txt"
RESULT_PATH = "/tmp/first-cloud-fleet.sha256"


async def run_tutorial(image_ref: str, pool_name: str) -> None:
    pool = None
    try:
        print(f"Cloud Fleet name: {pool_name}")
        pool = await Pool.apply(
            Image.from_registry(image_ref),
            name=pool_name,
            replicas=1,
            cpu=4,
            memory_mb=4096,
            services={"server": 8000},
        )

        async with pool.claim(
            name="first-task",
            service="server",
            time_to_start=900,
        ) as sandbox:
            await sandbox.files.write_text(SOURCE_PATH, SOURCE_TEXT)
            result = await sandbox.shell.run(
                f"sha256sum {SOURCE_PATH} | cut -d ' ' -f 1 > {RESULT_PATH}"
            )
            if not result.success:
                raise RuntimeError(result.stderr)

            guest_digest = (await sandbox.files.read_text(RESULT_PATH)).strip()
            local_digest = hashlib.sha256(SOURCE_TEXT.encode()).hexdigest()
            if guest_digest != local_digest:
                raise RuntimeError(
                    f"Verification failed: guest={guest_digest} local={local_digest}"
                )
            print(f"Verified SHA-256: {guest_digest}")
    finally:
        if pool is not None:
            print(f"Requesting deletion of Cloud Fleet: {pool_name}")
            await pool.delete()


async def main() -> None:
    image_ref = os.environ["CUA_FLEET_IMAGE"]
    pool_name = f"first-fleet-{uuid4().hex[:16]}"
    await run_tutorial(image_ref, pool_name)


if __name__ == "__main__":
    asyncio.run(main())
