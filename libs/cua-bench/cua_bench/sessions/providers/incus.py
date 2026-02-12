"""CUA Cloud V2 (Incus) session provider for batch benchmark execution on Incus VMs."""

import os
from pathlib import Path
from typing import Any, Dict, Optional

from .base import SessionProvider


class IncusProvider(SessionProvider):
    """Incus provider for running CUABench evaluations on Incus VMs.

    This provider uses the CUA Cloud V2 API (/v1/batch-jobs) to create
    IncusBatchJob CRDs that run solver containers inside Incus VMs.

    Each batch job:
    - Creates N Incus VMs (one per task)
    - Each VM runs cua-xfce (desktop) + solver container (benchmark agent)
    - Solver connects to desktop on localhost:8000
    - Results are polled via Incus files API

    Authentication:
        Requires CUA_API_KEY environment variable or ~/.cua/cli.sqlite credentials.
    """

    BASE_URL = "https://api.cua.cloud/v1"

    def __init__(self):
        self.api_key = self._get_api_key()
        self._http_client = None

    def _get_api_key(self) -> str:
        """Get API key from environment or stored credentials."""
        api_key = os.environ.get("CUA_API_KEY")
        if api_key:
            return api_key

        creds_path = Path.home() / ".cua" / "cli.sqlite"
        if creds_path.exists():
            try:
                import sqlite3

                conn = sqlite3.connect(str(creds_path))
                cursor = conn.cursor()
                cursor.execute("SELECT value FROM credentials WHERE key = 'api_key'")
                row = cursor.fetchone()
                conn.close()
                if row:
                    return row[0]
            except Exception:
                pass

        raise ValueError(
            "CUA Cloud API key not found. Set CUA_API_KEY environment variable "
            "or run 'cb auth login' to authenticate."
        )

    async def _get_http_client(self):
        """Get or create aiohttp client session."""
        if self._http_client is None:
            import aiohttp

            self._http_client = aiohttp.ClientSession(
                headers={
                    "Authorization": f"Bearer {self.api_key}",
                    "Content-Type": "application/json",
                }
            )
        return self._http_client

    async def _close_http_client(self):
        """Close HTTP client if open."""
        if self._http_client:
            await self._http_client.close()
            self._http_client = None

    async def start_session(
        self,
        session_id: str,
        env_path: Path,
        container_script: str,
        image_uri: Optional[str] = None,
        output_dir: Optional[str] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """Start a batch job on Incus VMs.

        Args:
            session_id: Unique identifier for the session
            env_path: Benchmark environment path
            container_script: Not used for Incus - solver config passed separately
            image_uri: Solver container image (default: trycua/cua-bench:latest)
            output_dir: Local directory to download results (optional)
            **kwargs: Additional arguments:
                - agent: Agent name
                - model: Model identifier
                - max_steps: Max steps per task
                - parallelism: Max concurrent VMs (default: 4)
                - repo_url: Git repo URL
                - ref: Git ref
                - vm_image: Incus VM base image (default: alpine-docker-cua)
                - task_count: Number of tasks (default: 1)
                - timeout_seconds: Per-task timeout (default: 600)

        Returns:
            Dict containing session metadata with batch job name
        """
        import aiohttp

        agent_name = kwargs.get("agent", "cua-agent")
        model = kwargs.get("model", "anthropic/claude-sonnet-4-20250514")
        max_steps = kwargs.get("max_steps", 50)
        parallelism = kwargs.get("parallelism", 4)
        task_count = kwargs.get("task_count", 1)
        timeout_seconds = kwargs.get("timeout_seconds", 600)
        vm_image = kwargs.get("vm_image", "alpine-docker-cua")
        solver_image = image_uri or "trycua/cua-bench:latest"

        # Build env vars from kwargs
        env_vars = {}
        for key in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GOOGLE_API_KEY"):
            val = os.environ.get(key)
            if val:
                env_vars[key] = val

        request_body: Dict[str, Any] = {
            "name": session_id,
            "image": vm_image,
            "solverImage": solver_image,
            "envPath": str(env_path),
            "taskCount": task_count,
            "parallelism": parallelism,
            "solverConfig": {
                "mode": "agent",
                "agentName": agent_name,
                "model": model,
                "maxSteps": max_steps,
                "timeoutSeconds": timeout_seconds,
            },
            "envVars": env_vars,
        }

        # Add source if repo_url provided
        repo_url = kwargs.get("repo_url")
        if repo_url:
            request_body["source"] = {
                "repoUrl": repo_url,
                "ref": kwargs.get("ref", "main"),
            }

        client = await self._get_http_client()

        try:
            async with client.post(
                f"{self.BASE_URL}/batch-jobs",
                json=request_body,
                timeout=aiohttp.ClientTimeout(total=30),
            ) as response:
                if response.status == 401:
                    raise ValueError("Invalid API key. Run 'cb auth login' to re-authenticate.")
                elif response.status == 429:
                    raise ValueError("Rate limited. Please wait and try again.")
                elif response.status != 201:
                    error_body = await response.text()
                    raise RuntimeError(
                        f"Failed to create batch job: {response.status} - {error_body}"
                    )

                result = await response.json()

        except aiohttp.ClientError as e:
            raise RuntimeError(f"Failed to connect to CUA Cloud: {e}")

        return {
            "session_id": session_id,
            "batch_job_name": result.get("name", session_id),
            "provider": "incus",
            "status": result.get("phase", "pending"),
            "env_path": str(env_path),
            "output_dir": output_dir,
            "task_summary": result.get("taskSummary", {}),
            "created_at": result.get("createdAt"),
        }

    async def get_session_status(self, session_id: str) -> Dict[str, Any]:
        """Get the status of a batch job.

        Args:
            session_id: Session identifier (used as batch job name)

        Returns:
            Dict containing session status
        """
        import aiohttp

        from ..manager import get_session

        session = get_session(session_id)
        if not session:
            return {"status": "not_found", "session_id": session_id}

        batch_name = session.get("batch_job_name", session_id)
        client = await self._get_http_client()

        try:
            async with client.get(
                f"{self.BASE_URL}/batch-jobs/{batch_name}",
                timeout=aiohttp.ClientTimeout(total=10),
            ) as response:
                if response.status == 404:
                    return {"status": "not_found", "session_id": session_id}
                elif response.status != 200:
                    return {"status": "error", "session_id": session_id}

                result = await response.json()

        except aiohttp.ClientError as e:
            return {"status": "error", "session_id": session_id, "error": str(e)}

        # Map batch job phase to local status
        phase = result.get("phase", "unknown")
        status_map = {
            "pending": "pending",
            "provisioning": "starting",
            "running": "running",
            "completed": "completed",
            "partiallycompleted": "completed",
            "failed": "failed",
            "cancelling": "stopping",
            "cancelled": "stopped",
        }

        return {
            "session_id": session_id,
            "batch_job_name": batch_name,
            "status": status_map.get(phase, phase),
            "task_summary": result.get("taskSummary", {}),
            "started_at": result.get("startedAt"),
        }

    async def stop_session(self, session_id: str) -> None:
        """Cancel a batch job.

        Args:
            session_id: Session identifier
        """
        import aiohttp

        from ..manager import get_session

        session = get_session(session_id)
        if not session:
            return

        batch_name = session.get("batch_job_name", session_id)
        client = await self._get_http_client()

        try:
            async with client.post(
                f"{self.BASE_URL}/batch-jobs/{batch_name}/cancel",
                timeout=aiohttp.ClientTimeout(total=10),
            ):
                pass
        except aiohttp.ClientError:
            pass

    async def get_session_logs(self, session_id: str, tail: Optional[int] = None) -> str:
        """Get status summary for a batch job.

        Args:
            session_id: Session identifier
            tail: Not used

        Returns:
            Status summary as text
        """
        status = await self.get_session_status(session_id)

        if status.get("status") == "not_found":
            return f"Session {session_id} not found"

        task_summary = status.get("task_summary", {})

        lines = [
            f"Batch Job Status: {status.get('status', 'unknown')}",
            f"Tasks: {task_summary.get('completed', 0)}/{task_summary.get('total', 0)} completed",
            f"Running: {task_summary.get('running', 0)}",
            f"Failed: {task_summary.get('failed', 0)}",
        ]

        return "\n".join(lines)

    async def get_results(self, session_id: str) -> Dict[str, Any]:
        """Get results from a batch job.

        Args:
            session_id: Session identifier

        Returns:
            Dict containing results
        """
        import aiohttp

        from ..manager import get_session

        session = get_session(session_id)
        if not session:
            return {"error": "Session not found"}

        batch_name = session.get("batch_job_name", session_id)
        client = await self._get_http_client()

        try:
            async with client.get(
                f"{self.BASE_URL}/batch-jobs/{batch_name}/results",
                timeout=aiohttp.ClientTimeout(total=30),
            ) as response:
                if response.status != 200:
                    return {"error": f"Failed to get results: {response.status}"}

                return await response.json()

        except aiohttp.ClientError as e:
            return {"error": str(e)}

    async def list_tasks(
        self,
        session_id: str,
        status: Optional[str] = None,
        limit: int = 50,
        offset: int = 0,
    ) -> Dict[str, Any]:
        """List task results for a batch job.

        For Incus batch jobs, tasks are embedded in the results endpoint.
        This method wraps get_results and formats as task list.

        Args:
            session_id: Session identifier
            status: Filter by task status
            limit: Max results
            offset: Pagination offset

        Returns:
            Dict containing tasks list
        """
        results = await self.get_results(session_id)

        if "error" in results:
            return {"error": results["error"], "tasks": []}

        tasks = results.get("results", [])

        # Apply status filter
        if status:
            tasks = [t for t in tasks if t.get("status") == status]

        # Apply pagination
        total = len(tasks)
        tasks = tasks[offset : offset + limit]

        return {
            "tasks": tasks,
            "total": total,
            "limit": limit,
            "offset": offset,
        }
