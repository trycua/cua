"""
Computer API package.
Provides a server interface for the Computer API.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

__version__: str = "0.1.0"

if TYPE_CHECKING:
    from .server import Server as Server

__all__ = ["Server", "run_cli"]


def run_cli() -> None:
    """Entry point for CLI"""
    from .cli import main

    main()


def __getattr__(name: str) -> Any:
    """Load the server only when callers explicitly request it.

    Keeping package import side-effect free lets the CLI select its backend
    before ``computer_server.main`` constructs the process-wide handlers.
    """

    if name == "Server":
        from .server import Server

        return Server
    raise AttributeError(name)
