#!/usr/bin/env python
"""
Entrypoint script for the Computer Server.

This script provides a simple way to start the Computer Server from the command line
or using a launch configuration in an IDE.

Usage:
    python run_server.py [--host HOST] [--port PORT] [--log-level LEVEL]
"""

import sys

from computer_server.cli import main

if __name__ == "__main__":
    sys.exit(main())
