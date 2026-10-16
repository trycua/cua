"""A worker started by create_workers keeps answering however much it logs.

uvicorn writes a line to the access log for every request. If nothing drains the
worker's output, the worker blocks once the OS pipe buffer (64 KiB, roughly 1,100
access-log lines) is full. This starts one real worker and sends well past that.
"""

import asyncio
import os
import signal
import sys

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


async def test_worker_output_reaches_parent_until_stopped(capfd):
    workers = await create_workers(n_workers=1, allowed_ips=["127.0.0.1"], host="127.0.0.1")
    try:
        async with aiohttp.ClientSession() as session:
            async with session.get(f"{workers[0].api_url}/health") as response:
                assert response.status == 200
    finally:
        await cleanup_workers(workers)

    assert not workers[0].output_reader.is_alive()
    err = capfd.readouterr().err
    assert "[w0] INFO:     Uvicorn running on" in err
    assert any(
        line.startswith("[w0] ") and '"GET /health HTTP/1.1" 200' in line
        for line in err.splitlines()
    )


class StderrFailingFirstWrite:
    def __init__(self):
        self.lines = []

    def write(self, line):
        self.lines.append(line)
        if len(self.lines) == 1:
            raise OSError("stderr unavailable")


async def test_worker_keeps_draining_when_forwarding_fails(monkeypatch):
    stderr = StderrFailingFirstWrite()
    monkeypatch.setattr(sys, "stderr", stderr)
    await test_worker_keeps_answering_past_pipe_buffer()
    assert len(stderr.lines) > N_REQUESTS


async def test_reader_stops_when_grandchild_holds_output_open(monkeypatch, tmp_path, capfd):
    grandchild_pid = tmp_path / "grandchild.pid"
    launcher = tmp_path / "python"
    launcher.write_text(
        f'#!/bin/sh\nsleep 60 &\necho $! > {grandchild_pid}\nexec {sys.executable} "$@"\n'
    )
    launcher.chmod(0o755)
    monkeypatch.setattr(sys, "executable", str(launcher))
    workers = await create_workers(n_workers=1, allowed_ips=["127.0.0.1"], host="127.0.0.1")
    monkeypatch.undo()
    try:
        await cleanup_workers(workers)
        assert not workers[0].output_reader.is_alive()
        assert "[w0] INFO:     Uvicorn running on" in capfd.readouterr().err
    finally:
        os.kill(int(grandchild_pid.read_text()), signal.SIGKILL)
