"""Base session provider interface."""

from abc import ABC, abstractmethod
from pathlib import Path
from typing import Any, Dict, Optional


class SessionProvider(ABC):
    """Base class for session providers (Docker, CUA Cloud, etc.)."""

    @abstractmethod
    async def start_session(
        self,
        session_id: str,
        env_path: Path,
        container_script: str,
        image_uri: Optional[str] = None,
        output_dir: Optional[str] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """Start a new session.

        Args:
            session_id: Unique identifier for the session
            env_path: Path to the environment directory
            container_script: Script to run in the container
            image_uri: Container image to use
            output_dir: Directory to save outputs
            **kwargs: Additional provider-specific arguments

        Returns:
            Dict containing session metadata (container_id, status, etc.)
        """
        pass

    @abstractmethod
    async def get_session_status(self, session_id: str) -> Dict[str, Any]:
        """Get the status of a running session.

        Args:
            session_id: Session identifier

        Returns:
            Dict containing session status information
        """
        pass

    @abstractmethod
    async def stop_session(self, session_id: str) -> None:
        """Stop a running session.

        Args:
            session_id: Session identifier
        """
        pass

    @abstractmethod
    async def get_session_logs(self, session_id: str, tail: Optional[int] = None) -> str:
        """Get logs from a session.

        Args:
            session_id: Session identifier
            tail: Number of lines to return from the end (None for all)

        Returns:
            Log output as string
        """
        pass
