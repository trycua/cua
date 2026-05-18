"""
Computer API package.
Provides a server interface for the Computer API.
"""

from __future__ import annotations

__version__: str = "0.1.0"

__all__ = ["Server", "run_cli"]


def __getattr__(name: str):
    if name == "Server":
        from .server import Server

        return Server
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def run_cli() -> None:
    """Entry point for CLI"""
    from .cli import main

    main()
