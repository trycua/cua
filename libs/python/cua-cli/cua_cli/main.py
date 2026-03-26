"""Main entry point for Cua CLI."""

import argparse
import logging
import sys
import time

from cua_cli import __version__
from cua_cli.commands import auth, do, image, mcp, platform, sandbox, skills, trajectory
from cua_cli.commands import workspace as workspace_cmd
from cua_cli.utils.output import print_error

try:
    from core.telemetry import is_telemetry_enabled, record_event

    _TELEMETRY_AVAILABLE = True
except ImportError:
    _TELEMETRY_AVAILABLE = False

    def is_telemetry_enabled() -> bool:  # type: ignore[misc]
        return False

    def record_event(event_name: str, properties: dict | None = None) -> None:  # type: ignore[misc]
        pass


def create_parser() -> argparse.ArgumentParser:
    """Create the main argument parser with all subcommands."""
    parser = argparse.ArgumentParser(
        prog="cua",
        description="Cua CLI - Unified command-line interface for Computer-Use Agents",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="For more information, visit https://docs.trycua.com",
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
    do.register_parser(subparsers)
    do.register_host_consent_parser(subparsers)
    trajectory.register_parser(subparsers)
    workspace_cmd.register_parser(subparsers)

    return parser


def main() -> int:
    """Main entry point for the CLI."""
    # Suppress noisy INFO logs from dependencies (computer, core.telemetry, etc.)
    # Must set on specific loggers since they configure their own handlers at import time
    logging.basicConfig(level=logging.WARNING)
    for name in ("computer", "core", "core.telemetry", "httpx", "httpcore", "cua_sandbox"):
        logging.getLogger(name).setLevel(logging.WARNING)

    parser = create_parser()
    args = parser.parse_args()

    if args.command is None:
        parser.print_help()
        return 0

    subcommand = getattr(args, "subcommand", None)
    _t_start = time.monotonic()
    exit_code = 0
    try:
        # Dispatch to command modules
        if args.command == "auth":
            exit_code = auth.execute(args)
        elif args.command in ("sandbox", "sb"):
            exit_code = sandbox.execute(args)
        elif args.command in ("image", "img"):
            exit_code = image.execute(args)
        elif args.command == "platform":
            exit_code = platform.execute(args)
        elif args.command == "skills":
            exit_code = skills.execute(args)
        elif args.command == "serve-mcp":
            exit_code = mcp.execute(args)
        elif args.command == "do":
            exit_code = do.execute(args)
        elif args.command == "do-host-consent":
            exit_code = do.execute_host_consent(args)
        elif args.command in ("trajectory", "traj"):
            exit_code = trajectory.execute(args)
        elif args.command in ("workspace", "ws"):
            exit_code = workspace_cmd.execute(args)
        else:
            print_error(f"Unknown command: {args.command}")
            exit_code = 1
        return exit_code
    except KeyboardInterrupt:
        print_error("Operation cancelled")
        exit_code = 130
        return exit_code
    except Exception as e:
        print_error(str(e))
        exit_code = 1
        return exit_code
    finally:
        if _TELEMETRY_AVAILABLE and is_telemetry_enabled():
            record_event(
                "cli_command",
                {
                    "command": args.command,
                    "subcommand": subcommand,
                    "status": "success" if exit_code == 0 else "error",
                    "exit_code": exit_code,
                    "duration_seconds": round(time.monotonic() - _t_start, 3),
                },
            )


if __name__ == "__main__":
    sys.exit(main())
