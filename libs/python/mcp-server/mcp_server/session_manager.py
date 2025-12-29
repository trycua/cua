"""
Session Manager for MCP Server - Handles concurrent client sessions with proper resource isolation.

This module provides:
- Per-session computer instance management
- Resource pooling and lifecycle management
- Graceful session cleanup
- Concurrent task execution support
"""

import asyncio
import logging
import os
import time
import uuid
import weakref
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Set

logger = logging.getLogger("mcp-server.session_manager")


@dataclass
class SessionInfo:
    """Information about an active session."""

    session_id: str
    computer: Any  # Computer instance
    created_at: float
    last_activity: float
    active_tasks: Set[str] = field(default_factory=set)
    is_shutting_down: bool = False


class ComputerPool:
    """Pool of computer instances for efficient resource management."""

    def __init__(self, max_size: int = 5, idle_timeout: float = 300.0):
        self.max_size = max_size
        self.idle_timeout = idle_timeout
        self._available: List[Any] = []
        self._in_use: Set[Any] = set()
        self._creation_lock = asyncio.Lock()

    async def acquire(self) -> Any:
        """Acquire a computer instance from the pool."""
        # Try to get an available instance
        if self._available:
            computer = self._available.pop()
            self._in_use.add(computer)
            logger.debug("Reusing computer instance from pool")
            return computer

        # Check if we can create a new one
        async with self._creation_lock:
            if len(self._in_use) < self.max_size:
                logger.debug("Creating new computer instance")
                from computer import Computer

                # Check if we should use host computer server
                use_host = os.getenv("CUA_USE_HOST_COMPUTER_SERVER", "false").lower() in (
                    "true",
                    "1",
                    "yes",
                )

                computer = Computer(verbosity=logging.INFO, use_host_computer_server=use_host)
                await computer.run()
                self._in_use.add(computer)
                return computer

        # Wait for an instance to become available
        logger.debug("Waiting for computer instance to become available")
        while not self._available:
            await asyncio.sleep(0.1)

        computer = self._available.pop()
        self._in_use.add(computer)
        return computer

    async def release(self, computer: Any) -> None:
        """Release a computer instance back to the pool."""
        if computer in self._in_use:
            self._in_use.remove(computer)
            self._available.append(computer)
            logger.debug("Released computer instance back to pool")

    async def cleanup_idle(self) -> None:
        """Clean up idle computer instances."""
        current_time = time.time()
        idle_instances = []

        for computer in self._available[:]:
            # Check if computer has been idle too long
            # Note: We'd need to track last use time per instance for this
            # For now, we'll keep instances in the pool
            pass

    async def shutdown(self) -> None:
        """Shutdown all computer instances in the pool."""
        logger.info("Shutting down computer pool")

        # Close all available instances
        for computer in self._available:
            try:
                if hasattr(computer, "close"):
                    await computer.close()
                elif hasattr(computer, "stop"):
                    await computer.stop()
            except Exception as e:
                logger.warning(f"Error closing computer instance: {e}")

        # Close all in-use instances
        for computer in self._in_use:
            try:
                if hasattr(computer, "close"):
                    await computer.close()
                elif hasattr(computer, "stop"):
                    await computer.stop()
            except Exception as e:
                logger.warning(f"Error closing computer instance: {e}")

        self._available.clear()
        self._in_use.clear()


class SessionManager:
    """Manages concurrent client sessions with proper resource isolation."""

    def __init__(self, max_concurrent_sessions: int = 10):
        self.max_concurrent_sessions = max_concurrent_sessions
        self._sessions: Dict[str, SessionInfo] = {}
        self._computer_pool = ComputerPool()
        self._session_lock = asyncio.Lock()
        self._cleanup_task: Optional[asyncio.Task] = None
        self._shutdown_event = asyncio.Event()

    async def start(self) -> None:
        """Start the session manager and cleanup task."""
        logger.info("Starting session manager")
        self._cleanup_task = asyncio.create_task(self._cleanup_loop())

    async def stop(self) -> None:
        """Stop the session manager and cleanup all resources."""
        logger.info("Stopping session manager")
        self._shutdown_event.set()

        if self._cleanup_task:
            self._cleanup_task.cancel()
            try:
                await self._cleanup_task
            except asyncio.CancelledError:
                pass

        # Force cleanup all sessions
        async with self._session_lock:
            session_ids = list(self._sessions.keys())

        for session_id in session_ids:
            await self._force_cleanup_session(session_id)

        await self._computer_pool.shutdown()

    @asynccontextmanager
    async def get_session(self, session_id: Optional[str] = None) -> Any:
        """Get or create a session with proper resource management."""
        if session_id is None:
            session_id = str(uuid.uuid4())

        # Check if session exists and is not shutting down
        async with self._session_lock:
            if session_id in self._sessions:
                session = self._sessions[session_id]
                if session.is_shutting_down:
                    raise RuntimeError(f"Session {session_id} is shutting down")
                session.last_activity = time.time()
                computer = session.computer
            else:
                # Create new session
                if len(self._sessions) >= self.max_concurrent_sessions:
                    raise RuntimeError(
                        f"Maximum concurrent sessions ({self.max_concurrent_sessions}) reached"
                    )

                computer = await self._computer_pool.acquire()
                session = SessionInfo(
                    session_id=session_id,
                    computer=computer,
                    created_at=time.time(),
                    last_activity=time.time(),
                )
                self._sessions[session_id] = session
                logger.info(f"Created new session: {session_id}")

        try:
            yield session
        finally:
            # Update last activity
            async with self._session_lock:
                if session_id in self._sessions:
                    self._sessions[session_id].last_activity = time.time()

    async def register_task(self, session_id: str, task_id: str) -> None:
        """Register a task for a session."""
        async with self._session_lock:
            if session_id in self._sessions:
                self._sessions[session_id].active_tasks.add(task_id)
                logger.debug(f"Registered task {task_id} for session {session_id}")

    async def unregister_task(self, session_id: str, task_id: str) -> None:
        """Unregister a task from a session."""
        async with self._session_lock:
            if session_id in self._sessions:
                self._sessions[session_id].active_tasks.discard(task_id)
                logger.debug(f"Unregistered task {task_id} from session {session_id}")

    async def cleanup_session(self, session_id: str) -> None:
        """Cleanup a specific session."""
        async with self._session_lock:
            if session_id not in self._sessions:
                return

            session = self._sessions[session_id]

            # Check if session has active tasks
            if session.active_tasks:
                logger.info(f"Session {session_id} has active tasks, marking for shutdown")
                session.is_shutting_down = True
                return

            # Actually cleanup the session
            await self._force_cleanup_session(session_id)

    async def _force_cleanup_session(self, session_id: str) -> None:
        """Force cleanup a session regardless of active tasks."""
        async with self._session_lock:
            if session_id not in self._sessions:
                return

            session = self._sessions[session_id]
            logger.info(f"Cleaning up session: {session_id}")

            # Release computer back to pool
            await self._computer_pool.release(session.computer)

            # Remove session
            del self._sessions[session_id]

    async def _cleanup_loop(self) -> None:
        """Background task to cleanup idle sessions."""
        while not self._shutdown_event.is_set():
            try:
                await asyncio.sleep(60)  # Run cleanup every minute

                current_time = time.time()
                idle_timeout = 600.0  # 10 minutes

                async with self._session_lock:
                    idle_sessions = []
                    for session_id, session in self._sessions.items():
                        if not session.is_shutting_down and not session.active_tasks:
                            if current_time - session.last_activity > idle_timeout:
                                idle_sessions.append(session_id)

                # Cleanup idle sessions
                for session_id in idle_sessions:
                    await self._force_cleanup_session(session_id)
                    logger.info(f"Cleaned up idle session: {session_id}")

            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"Error in cleanup loop: {e}")

    def get_session_stats(self) -> Dict[str, Any]:
        """Get statistics about active sessions."""

        async def _get_stats():
            async with self._session_lock:
                return {
                    "total_sessions": len(self._sessions),
                    "max_concurrent": self.max_concurrent_sessions,
                    "sessions": {
                        session_id: {
                            "created_at": session.created_at,
                            "last_activity": session.last_activity,
                            "active_tasks": len(session.active_tasks),
                            "is_shutting_down": session.is_shutting_down,
                        }
                        for session_id, session in self._sessions.items()
                    },
                }

        # Run in current event loop or create new one
        try:
            loop = asyncio.get_running_loop()
            return asyncio.run_coroutine_threadsafe(_get_stats(), loop).result()
        except RuntimeError:
            # No event loop running, create a new one
            return asyncio.run(_get_stats())


# Global session manager instance
_session_manager: Optional[SessionManager] = None


def get_session_manager() -> SessionManager:
    """Get the global session manager instance."""
    global _session_manager
    if _session_manager is None:
        _session_manager = SessionManager()
    return _session_manager


async def initialize_session_manager() -> None:
    """Initialize the global session manager."""
    global _session_manager
    if _session_manager is None:
        _session_manager = SessionManager()
        await _session_manager.start()
    return _session_manager


async def shutdown_session_manager() -> None:
    """Shutdown the global session manager."""
    global _session_manager
    if _session_manager is not None:
        await _session_manager.stop()
        _session_manager = None
