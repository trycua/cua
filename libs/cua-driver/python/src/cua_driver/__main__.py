"""CLI entry point for cua-driver Python wrapper.

This module is invoked when running:
    python -m cua_driver [args...]
or via the installed script:
    cua-driver [args...]
"""

import sys
from .wrapper import run_cua_driver


def main() -> None:
    """Main entry point for the cua-driver CLI."""
    exit_code = run_cua_driver()
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
