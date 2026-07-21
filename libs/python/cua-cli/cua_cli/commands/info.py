"""Diagnostic information command for Cua CLI."""

import argparse
import platform as sys_platform
import sys

from cua_cli import __version__


def collect_diagnostics() -> dict[str, str]:
    """Collect deterministic environment diagnostic fields."""

    return {
        "cua_cli_version": __version__,
        "python_version": sys.version.split(" ")[0],
        "platform": sys_platform.platform(),
        "system": sys_platform.system(),
        "release": sys_platform.release(),
        "machine": sys_platform.machine(),
    }


def _print_diagnostics(diagnostics: dict[str, str]) -> None:
    """Print collected diagnostics in a stable, testable format."""

    print("Environment Diagnostics")
    print("-" * 24)
    print(f"cua-cli version: {diagnostics['cua_cli_version']}")
    print(f"python version: {diagnostics['python_version']}")
    print(f"platform: {diagnostics['platform']}")
    print(f"system: {diagnostics['system']}")
    print(f"release: {diagnostics['release']}")
    print(f"machine/architecture: {diagnostics['machine']}")


def execute(_args: argparse.Namespace) -> int:
    """Execute the info command and print diagnostics."""

    diagnostics = collect_diagnostics()
    _print_diagnostics(diagnostics)
    return 0


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the top-level `info` command."""

    subparsers.add_parser(
        "info",
        help="Print environment diagnostics for bug reports",
        description="Print environment diagnostics for bug reports and debugging.",
    )
