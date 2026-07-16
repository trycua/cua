"""Python wrapper for cua-driver - cross-platform MCP server.

This package provides a thin Python wrapper around the cua-driver Rust binary,
enabling pip-installable access to the MCP server for computer-use automation.
"""

__version__ = "0.8.3"  # x-release-please-version

from .wrapper import run_cua_driver, get_binary_path

__all__ = ["run_cua_driver", "get_binary_path", "__version__"]
