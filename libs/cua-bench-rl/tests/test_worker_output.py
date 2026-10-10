"""A worker started by create_workers keeps answering however much it logs.

uvicorn writes a line to the access log for every request. If nothing drains the
worker's output, the worker blocks once the OS pipe buffer (64 KiB, roughly 1,100
access-log lines) is full. This starts one real worker and sends well past that.
"""

import asyncio

import aiohttp
import pytest
from cua_bench_rl.workers.worker_manager import cleanup_workers, create_workers

N_REQUESTS = 4000
REQUEST_TIMEOUT_S = 5.0


async def test_worker_keeps_answering_past_pipe_buffer():
    workers = await create_workers(n_workers=1, allowed_ips=["127.0.0.1"], host="127.0.0.1")
    try:
        url = f"{workers[0].api_url}/health"
        timeout = aiohttp.ClientTimeout(total=REQUEST_TIMEOUT_S)
        async with aiohttp.ClientSession(timeout=timeout) as session:
            for i in range(N_REQUESTS):
                try:
                    async with session.get(url) as response:
                        assert response.status == 200
                except asyncio.TimeoutError:
                    pytest.fail(f"worker stopped answering after {i} requests")
    finally:
        await cleanup_workers(workers)
