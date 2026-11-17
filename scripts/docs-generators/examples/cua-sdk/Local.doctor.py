# docs: test="docs"
import asyncio

import cua


async def main():
    report = await cua.embedded().local().doctor()
    for check in report.checks:
        print(check.name, check.status)


asyncio.run(main())
