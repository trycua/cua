"""CUA Cloud session provider for remote evaluation execution."""

import os
import json
import time
from typing import Dict, Any, Optional, List
from pathlib import Path

from .base import SessionProvider


class CloudProvider(SessionProvider):
    """CUA Cloud provider for remote evaluation execution.

    This provider uses the CUA Cloud Evals API to run tasks on:
    - GCP Batch for webtop (Linux) tasks
    - Azure Batch for Windows Arena tasks

    The cloud backend auto-detects task type from env_path and routes accordingly.

    Authentication:
        Requires CUA_API_KEY environment variable or ~/.cua/cli.sqlite credentials.
    """

    BASE_URL = "https://api.cua.cloud/v1"

    def __init__(self):
        self.api_key = self._get_api_key()
        self._http_client = None

    def _get_api_key(self) -> str:
        """Get API key from environment or stored credentials."""
        # Check environment variable first
        api_key = os.environ.get("CUA_API_KEY")
        if api_key:
            return api_key

        # Check stored credentials in ~/.cua/cli.sqlite
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
        **kwargs
    ) -> Dict[str, Any]:
        """Start a cloud evaluation session.

        This creates an eval on CUA Cloud which runs the task(s) on
        GCP Batch (webtop) or Azure Batch (Windows Arena).

        Args:
            session_id: Unique identifier for the session
            env_path: Path to the environment directory
            container_script: Not used for cloud - agent config passed separately
            image_uri: Not used for cloud - backend selects appropriate image
            output_dir: Local directory to download results (optional)
            **kwargs: Additional arguments:
                - agent: Agent name (e.g., "cua-agent", "gemini")
                - model: Model identifier (e.g., "anthropic/claude-sonnet-4-20250514")
                - max_steps: Max steps per task
                - task_index: Specific task to run (optional)
                - parallelism: Max concurrent tasks (default: 10)
                - repo_url: Git repo URL (if env is from a repo)
                - ref: Git ref (branch/tag/commit)

        Returns:
            Dict containing session metadata with eval_id
        """
        import aiohttp

        # Build eval request
        agent_name = kwargs.get("agent", "cua-agent")
        model = kwargs.get("model", "anthropic/claude-sonnet-4-20250514")
        max_steps = kwargs.get("max_steps", 50)
        task_index = kwargs.get("task_index")
        parallelism = kwargs.get("parallelism", 10)

        # Determine source configuration
        # If repo_url provided, use it; otherwise assume local path
        repo_url = kwargs.get("repo_url")
        ref = kwargs.get("ref", "main")

        if repo_url:
            source = {
                "repo_url": repo_url,
                "ref": ref,
                "env_path": str(env_path),
            }
        else:
            # For local paths, we need to determine the repo URL
            # This is a simplified approach - in practice, you'd want to
            # detect the git remote or require explicit repo_url
            source = {
                "repo_url": "https://github.com/trycua/cua-bench.git",
                "ref": "main",
                "env_path": str(env_path),
            }

        request_body = {
            "name": session_id,
            "source": source,
            "agent_config": {
                "name": agent_name,
                "model": model,
                "max_steps": max_steps,
            },
            "execution_config": {
                "parallelism": parallelism,
                "timeout_per_task_seconds": 600,
                "retry_attempts": 2,
            },
            "output_config": {
                "capture_screenshots": "final",
                "capture_traces": True,
            },
        }

        # If specific task requested, add task_ids
        if task_index is not None:
            request_body["task_ids"] = [str(task_index)]

        # Make API request
        client = await self._get_http_client()

        try:
            async with client.post(
                f"{self.BASE_URL}/evals",
                json=request_body,
                timeout=aiohttp.ClientTimeout(total=30),
            ) as response:
                if response.status == 401:
                    raise ValueError("Invalid API key. Run 'cb auth login' to re-authenticate.")
                elif response.status == 402:
                    raise ValueError("Usage quota exceeded. Check your CUA Cloud plan.")
                elif response.status == 429:
                    raise ValueError("Rate limited. Please wait and try again.")
                elif response.status != 201:
                    error_body = await response.text()
                    raise RuntimeError(f"Failed to create eval: {response.status} - {error_body}")

                result = await response.json()

        except aiohttp.ClientError as e:
            raise RuntimeError(f"Failed to connect to CUA Cloud: {e}")

        eval_id = result["eval_id"]

        return {
            "session_id": session_id,
            "eval_id": eval_id,
            "provider": "cloud",
            "status": result.get("status", "pending"),
            "env_path": str(env_path),
            "output_dir": output_dir,
            "task_summary": result.get("task_summary", {}),
            "created_at": result.get("created_at"),
        }

    async def get_session_status(self, session_id: str) -> Dict[str, Any]:
        """Get the status of a cloud evaluation.

        Args:
            session_id: Session identifier

        Returns:
            Dict containing session status
        """
        import aiohttp

        # Get eval_id from session storage
        from ..manager import get_session
        session = get_session(session_id)

        if not session:
            return {"status": "not_found", "session_id": session_id}

        eval_id = session.get("eval_id")
        if not eval_id:
            return {"status": "error", "session_id": session_id, "error": "No eval_id found"}

        client = await self._get_http_client()

        try:
            async with client.get(
                f"{self.BASE_URL}/evals/{eval_id}",
                timeout=aiohttp.ClientTimeout(total=10),
            ) as response:
                if response.status == 404:
                    return {"status": "not_found", "session_id": session_id}
                elif response.status != 200:
                    return {"status": "error", "session_id": session_id}

                result = await response.json()

        except aiohttp.ClientError as e:
            return {"status": "error", "session_id": session_id, "error": str(e)}

        # Map cloud status to local status
        cloud_status = result.get("status", "unknown")
        status_map = {
            "pending": "pending",
            "provisioning": "starting",
            "running": "running",
            "completed": "completed",
            "partially_completed": "completed",
            "failed": "failed",
            "cancelled": "stopped",
        }

        return {
            "session_id": session_id,
            "eval_id": eval_id,
            "status": status_map.get(cloud_status, cloud_status),
            "task_summary": result.get("task_summary", {}),
            "metrics": result.get("metrics", {}),
            "started_at": result.get("started_at"),
        }

    async def stop_session(self, session_id: str) -> None:
        """Cancel a cloud evaluation.

        Args:
            session_id: Session identifier
        """
        import aiohttp

        from ..manager import get_session
        session = get_session(session_id)

        if not session:
            return

        eval_id = session.get("eval_id")
        if not eval_id:
            return

        client = await self._get_http_client()

        try:
            async with client.post(
                f"{self.BASE_URL}/evals/{eval_id}/cancel",
                timeout=aiohttp.ClientTimeout(total=10),
            ) as response:
                # Ignore errors - best effort cancellation
                pass
        except aiohttp.ClientError:
            pass

    async def get_session_logs(self, session_id: str, tail: Optional[int] = None) -> str:
        """Get logs from a cloud evaluation.

        Note: Cloud evals don't have real-time logs. Use get_results() instead.

        Args:
            session_id: Session identifier
            tail: Not used for cloud

        Returns:
            Status message or task summary
        """
        status = await self.get_session_status(session_id)

        if status.get("status") == "not_found":
            return f"Session {session_id} not found"

        task_summary = status.get("task_summary", {})
        metrics = status.get("metrics", {})

        lines = [
            f"Eval Status: {status.get('status', 'unknown')}",
            f"Tasks: {task_summary.get('completed', 0)}/{task_summary.get('total', 0)} completed",
            f"Running: {task_summary.get('running', 0)}",
            f"Failed: {task_summary.get('failed', 0)}",
        ]

        if metrics:
            if "average_reward" in metrics:
                lines.append(f"Average Reward: {metrics['average_reward']:.2f}")
            if "average_task_duration_seconds" in metrics:
                lines.append(f"Avg Duration: {metrics['average_task_duration_seconds']:.1f}s")

        return "\n".join(lines)

    async def get_results(self, session_id: str) -> Dict[str, Any]:
        """Get aggregated results from a completed cloud evaluation.

        Args:
            session_id: Session identifier

        Returns:
            Dict containing results summary and artifact URLs
        """
        import aiohttp

        from ..manager import get_session
        session = get_session(session_id)

        if not session:
            return {"error": "Session not found"}

        eval_id = session.get("eval_id")
        if not eval_id:
            return {"error": "No eval_id found"}

        client = await self._get_http_client()

        try:
            async with client.get(
                f"{self.BASE_URL}/evals/{eval_id}/results",
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
        """List task results for a cloud evaluation.

        Args:
            session_id: Session identifier
            status: Filter by task status
            limit: Max results
            offset: Pagination offset

        Returns:
            Dict containing tasks list
        """
        import aiohttp

        from ..manager import get_session
        session = get_session(session_id)

        if not session:
            return {"error": "Session not found", "tasks": []}

        eval_id = session.get("eval_id")
        if not eval_id:
            return {"error": "No eval_id found", "tasks": []}

        client = await self._get_http_client()

        params = {"limit": limit, "offset": offset}
        if status:
            params["status"] = status

        try:
            async with client.get(
                f"{self.BASE_URL}/evals/{eval_id}/tasks",
                params=params,
                timeout=aiohttp.ClientTimeout(total=30),
            ) as response:
                if response.status != 200:
                    return {"error": f"Failed to get tasks: {response.status}", "tasks": []}

                return await response.json()

        except aiohttp.ClientError as e:
            return {"error": str(e), "tasks": []}
