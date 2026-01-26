"""Worker manager for spawning and managing multiple worker servers.

This module provides utilities for creating and managing pools of worker servers,
enabling parallel environment execution.

Example:
    workers = await create_workers(
        n_workers=4,
        allowed_ips=["127.0.0.1", "10.0.0.5"],
    )
    # Each worker manages up to 2 envs, so 4 workers = 8 parallel envs

    # Use workers...

    await cleanup_workers(workers)
"""

import asyncio
import os
import socket
import subprocess
import sys
from dataclasses import dataclass
from typing import List, Optional

import aiohttp


def _find_free_ports(n: int) -> List[int]:
    """Find N free ports by binding to port 0.

    Args:
        n: Number of free ports to find

    Returns:
        List of available port numbers
    """
    ports = []
    sockets = []

    for _ in range(n):
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("", 0))
        ports.append(s.getsockname()[1])
        sockets.append(s)

    # Close all sockets to release the ports
    for s in sockets:
        s.close()

    return ports


@dataclass
class WorkerHandle:
    """Handle for a running worker server.

    Attributes:
        worker_id: Unique identifier for this worker
        port: Port the worker is listening on
        process: Subprocess running the worker
        api_url: Full URL for API requests
    """

    worker_id: str
    port: int
    process: subprocess.Popen
    api_url: str

    async def health_check(self, timeout: float = 5.0) -> bool:
        """Check if the worker is healthy.

        Args:
            timeout: Request timeout in seconds

        Returns:
            True if healthy, False otherwise
        """
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(
                    f"{self.api_url}/health",
                    timeout=aiohttp.ClientTimeout(total=timeout),
                ) as response:
                    if response.status == 200:
                        return True
        except Exception:
            pass
        return False

    def stop(self) -> None:
        """Stop the worker process."""
        if self.process and self.process.poll() is None:
            # Try graceful shutdown first
            try:
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    # Force kill if graceful shutdown fails
                    self.process.kill()
                    self.process.wait(timeout=5)
            except Exception:
                pass

    @property
    def is_running(self) -> bool:
        """Check if the worker process is still running."""
        return self.process is not None and self.process.poll() is None


async def create_workers(
    n_workers: int,
    allowed_ips: List[str],
    startup_timeout: float = 30.0,
    host: str = "0.0.0.0",
) -> List[WorkerHandle]:
    """Spawn N worker servers on automatically allocated free ports.

    Args:
        n_workers: Number of worker servers to spawn
        allowed_ips: List of IPs allowed to access workers
        startup_timeout: Max time to wait for each worker to become healthy
        host: Host for workers to bind to

    Returns:
        List of WorkerHandle objects

    Raises:
        RuntimeError: If any worker fails to start

    Example:
        workers = await create_workers(
            n_workers=4,
            allowed_ips=["127.0.0.1", "10.0.0.5"],
        )
        # Each worker manages up to 2 envs, so 4 workers = 8 parallel envs
    """
    handles: List[WorkerHandle] = []

    # Find free ports for all workers
    ports = _find_free_ports(n_workers)

    for i in range(n_workers):
        port = ports[i]
        worker_id = f"w{i}"

        # Build environment
        env = os.environ.copy()
        env["OSGYM_ALLOWED_IPS"] = ",".join(allowed_ips)

        # Start worker process
        cmd = [
            sys.executable,
            "-m",
            "cua_bench.workers.worker_server",
            "--host",
            host,
            "--port",
            str(port),
        ]

        try:
            process = subprocess.Popen(
                cmd,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,  # Detach from parent
            )
        except Exception as e:
            # Clean up already started workers
            for handle in handles:
                handle.stop()
            raise RuntimeError(f"Failed to start worker {worker_id}: {e}")

        api_url = f"http://127.0.0.1:{port}"
        handle = WorkerHandle(
            worker_id=worker_id,
            port=port,
            process=process,
            api_url=api_url,
        )
        handles.append(handle)

    # Wait for all workers to become healthy
    async def wait_for_health(handle: WorkerHandle) -> bool:
        """Wait for a worker to become healthy."""
        start_time = asyncio.get_event_loop().time()
        while asyncio.get_event_loop().time() - start_time < startup_timeout:
            if await handle.health_check():
                return True
            await asyncio.sleep(0.5)
        return False

    try:
        health_results = await asyncio.gather(
            *[wait_for_health(h) for h in handles],
            return_exceptions=True,
        )

        failed_workers = []
        for i, (handle, result) in enumerate(zip(handles, health_results)):
            if isinstance(result, Exception) or not result:
                failed_workers.append(handle.worker_id)

        if failed_workers:
            # Clean up all workers
            for handle in handles:
                handle.stop()
            failed_ports = [handles[int(w[1:])].port for w in failed_workers]
            raise RuntimeError(
                f"Workers failed to start: {failed_workers} on ports {failed_ports}."
            )

    except Exception:
        # Clean up all workers on any error
        for handle in handles:
            handle.stop()
        raise

    return handles


async def cleanup_workers(workers: List[WorkerHandle]) -> None:
    """Stop all workers.

    Args:
        workers: List of WorkerHandle objects to stop
    """
    for handle in workers:
        handle.stop()


async def wait_for_workers(
    workers: List[WorkerHandle],
    timeout: Optional[float] = None,
) -> None:
    """Wait for all workers to terminate.

    Args:
        workers: List of WorkerHandle objects to wait for
        timeout: Max time to wait (None for indefinite)

    Raises:
        asyncio.TimeoutError: If timeout expires before all workers terminate
    """

    async def wait_for_one(handle: WorkerHandle) -> None:
        """Wait for a single worker to terminate."""
        while handle.is_running:
            await asyncio.sleep(0.1)

    wait_tasks = [wait_for_one(h) for h in workers]

    if timeout is not None:
        await asyncio.wait_for(
            asyncio.gather(*wait_tasks),
            timeout=timeout,
        )
    else:
        await asyncio.gather(*wait_tasks)


class WorkerPool:
    """Context manager for a pool of worker servers.

    Example:
        async with WorkerPool(n_workers=4, allowed_ips=["127.0.0.1"]) as pool:
            for url in pool.urls:
                client = CBEnvWorkerClient(server_url=url)
                # Use client...
    """

    def __init__(
        self,
        n_workers: int,
        allowed_ips: List[str],
        startup_timeout: float = 30.0,
        host: str = "0.0.0.0",
    ):
        self.n_workers = n_workers
        self.allowed_ips = allowed_ips
        self.startup_timeout = startup_timeout
        self.host = host
        self._workers: List[WorkerHandle] = []

    async def __aenter__(self) -> "WorkerPool":
        self._workers = await create_workers(
            n_workers=self.n_workers,
            allowed_ips=self.allowed_ips,
            startup_timeout=self.startup_timeout,
            host=self.host,
        )
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        await cleanup_workers(self._workers)

    @property
    def workers(self) -> List[WorkerHandle]:
        """Get the list of worker handles."""
        return self._workers

    @property
    def urls(self) -> List[str]:
        """Get the list of worker URLs."""
        return [w.api_url for w in self._workers]

    async def health_check_all(self) -> dict:
        """Check health of all workers.

        Returns:
            Dict mapping worker_id to health status
        """
        results = {}
        for worker in self._workers:
            results[worker.worker_id] = await worker.health_check()
        return results
