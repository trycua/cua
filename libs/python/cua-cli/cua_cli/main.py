"""Main entry point for CUA CLI."""

import argparse
import logging
import sys

from cua_cli import __version__
from cua_cli.commands import auth, image, mcp, platform, sandbox, skills
from cua_cli.utils.output import print_error


def create_parser() -> argparse.ArgumentParser:
    """Create the main argument parser with all subcommands."""
    parser = argparse.ArgumentParser(
        prog="cua",
        description="CUA CLI - Unified command-line interface for Computer-Use Agents",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  cua auth login              Authenticate via browser
  cua auth login --api-key    Authenticate with API key
  cua sb list                 List all sandboxes
  cua sb create --os linux    Create a new Linux sandbox
  cua image list              List cloud images
  cua image list --local      List local images
  cua image create linux-docker   Create a local image
  cua image shell <name>      Interactive shell into image
  cua platform list           Show available platforms

For more information, visit https://docs.trycua.com
""",
    )

    parser.add_argument(
        "-v",
        "--version",
        action="version",
        version=f"cua {__version__}",
    )

    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Register command modules
    auth.register_parser(subparsers)
    sandbox.register_parser(subparsers)
    image.register_parser(subparsers)
    platform.register_parser(subparsers)
    skills.register_parser(subparsers)
    mcp.register_parser(subparsers)

    return parser


def main() -> int:
    """Main entry point for the CLI."""
    # Suppress noisy INFO logs from dependencies (computer, core.telemetry, etc.)
    # Must set on specific loggers since they configure their own handlers at import time
    logging.basicConfig(level=logging.WARNING)
    for name in ("computer", "core", "core.telemetry"):
        logging.getLogger(name).setLevel(logging.WARNING)

    parser = create_parser()
    args = parser.parse_args()

    if args.command is None:
        parser.print_help()
        return 0

    try:
        # Dispatch to command modules
        if args.command == "auth":
            return auth.execute(args)
        elif args.command in ("sandbox", "sb"):
            return sandbox.execute(args)
        elif args.command in ("image", "img"):
            return image.execute(args)
        elif args.command == "platform":
            return platform.execute(args)
        elif args.command == "skills":
            return skills.execute(args)
        elif args.command == "serve-mcp":
            return mcp.execute(args)
        else:
            print_error(f"Unknown command: {args.command}")
            return 1
    except KeyboardInterrupt:
        print_error("Operation cancelled")
        return 130
    except Exception as e:
        print_error(str(e))
        return 1


if __name__ == "__main__":
    sys.exit(main())
