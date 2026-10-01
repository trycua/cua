"""
Entry point for running agent CLI module.

Usage:
    python -m agent.cli <model_string>
"""

import asyncio
import sys

from .cli import main

if __name__ == "__main__":
    # Check if 'cli' is specified as the module
    if len(sys.argv) > 1 and sys.argv[1] == "cli":
        # Remove 'cli' from arguments and run CLI
        sys.argv.pop(1)
        asyncio.run(main())
    else:
        print("Usage: python -m agent.cli <model_string>")
        print("Example: python -m agent.cli openai/computer-use-preview")
        sys.exit(1)
