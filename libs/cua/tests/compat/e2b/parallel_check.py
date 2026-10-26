"""N sandboxes at once through one SDK client: create, run a command, write
and read a file, kill. A plain E2B program written this way (asyncio.gather of
AsyncSandbox.create + commands.run) must work against the gateway; a failure
in any of them fails the check.

    python parallel_check.py [N]          # default 16; E2B_* from the environment
"""

import asyncio
import sys
import time

from e2b import AsyncSandbox


async def one(i: int) -> str:
    sb = await AsyncSandbox.create(timeout=300)
    try:
        r = await sb.commands.run(f"echo sandbox-{i}-$((6*7))")
        await sb.files.write(f"/tmp/p{i}.txt", f"data-{i}")
        back = await sb.files.read(f"/tmp/p{i}.txt")
        assert back == f"data-{i}", back
        return r.stdout.strip()
    finally:
        await sb.kill()


async def main(n: int) -> int:
    t0 = time.time()
    res = await asyncio.gather(*(one(i) for i in range(n)), return_exceptions=True)
    bad = [(i, r) for i, r in enumerate(res) if isinstance(r, BaseException)]
    for i, r in bad:
        print(f"FAIL {i}: {type(r).__name__}: {r}", file=sys.stderr)
    wrong = [
        i for i, r in enumerate(res) if not isinstance(r, BaseException) and r != f"sandbox-{i}-42"
    ]
    print(
        f"{n} parallel: {n - len(bad)} ok, {len(bad)} failed, {len(wrong)} wrong output, {time.time() - t0:.0f}s"
    )
    return 1 if bad or wrong else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main(int(sys.argv[1]) if len(sys.argv) > 1 else 16)))
