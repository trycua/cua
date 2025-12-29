"""
Main entry point for running the Computer Server as a module.
This allows the server to be started with `python -m computer_server`.
"""

import sys

from .cli import main

if __name__ == "__main__":
    sys.exit(main())
