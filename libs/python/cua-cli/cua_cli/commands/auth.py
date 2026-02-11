"""Authentication commands for CUA CLI."""

import argparse
import os
from pathlib import Path
from typing import Any, Optional

import aiohttp
from cua_cli.auth.browser import authenticate_via_browser
from cua_cli.auth.store import clear_credentials, get_api_key, save_api_key
from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import print_error, print_info, print_success

DEFAULT_API_BASE = "https://api.cua.ai"


def _get_api_base() -> str:
    return os.environ.get("CUA_API_BASE", DEFAULT_API_BASE).rstrip("/")


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the auth command and subcommands.

    Args:
        subparsers: The subparsers object from the main parser
    """
    auth_parser = subparsers.add_parser(
        "auth",
        help="Authentication commands",
        description="Manage authentication for CUA cloud services",
    )

    auth_subparsers = auth_parser.add_subparsers(
        dest="auth_command",
        help="Authentication command",
    )

    # login command
    login_parser = auth_subparsers.add_parser(
        "login",
        help="Authenticate with CUA cloud",
        description="Authenticate via browser or API key",
    )
    login_parser.add_argument(
        "--api-key",
        type=str,
        help="API key for direct authentication (skips browser flow)",
    )

    # logout command
    auth_subparsers.add_parser(
        "logout",
        help="Clear stored credentials",
        description="Remove all stored authentication credentials",
    )

    # status command
    auth_subparsers.add_parser(
        "status",
        help="Show authentication status and account info",
        description="Display current user, credits, and API key info",
    )

    # env command
    env_parser = auth_subparsers.add_parser(
        "env",
        help="Export API key to .env file",
        description="Write CUA_API_KEY to .env file in current directory",
    )
    env_parser.add_argument(
        "--file",
        type=str,
        default=".env",
        help="Path to .env file (default: .env)",
    )


def execute(args: argparse.Namespace) -> int:
    """Execute auth command based on subcommand.

    Args:
        args: Parsed command-line arguments

    Returns:
        Exit code (0 for success, non-zero for failure)
    """
    cmd = getattr(args, "auth_command", None)

    if cmd == "login":
        return cmd_login(args)
    elif cmd == "logout":
        return cmd_logout(args)
    elif cmd == "status":
        return cmd_status(args)
    elif cmd == "env":
        return cmd_env(args)
    else:
        print_error("Usage: cua auth <command>")
        print_info("Commands: login, logout, status, env")
        return 1


def cmd_login(args: argparse.Namespace) -> int:
    """Handle the login command.

    Args:
        args: Parsed command-line arguments

    Returns:
        Exit code
    """
    # Check if already logged in
    existing_key = get_api_key()
    if existing_key and not args.api_key:
        print_info("Already authenticated. Use 'cua auth logout' to clear credentials.")
        return 0

    if args.api_key:
        # Direct API key authentication
        api_key = args.api_key
        print_info("Authenticating with provided API key...")
    else:
        # Browser-based authentication
        try:
            api_key = run_async(authenticate_via_browser())
        except TimeoutError as e:
            print_error(str(e))
            return 1
        except RuntimeError as e:
            print_error(str(e))
            return 1

    # Save the API key
    save_api_key(api_key)
    print_success("Successfully authenticated!")

    return 0


def cmd_logout(args: argparse.Namespace) -> int:
    """Handle the logout command.

    Args:
        args: Parsed command-line arguments

    Returns:
        Exit code
    """
    clear_credentials()
    print_success("Credentials cleared.")
    return 0


def cmd_status(args: argparse.Namespace) -> int:
    """Handle the status command — show auth status and account info."""
    api_key = get_api_key()
    if not api_key:
        print_error("Not logged in. Run 'cua auth login' first.")
        return 1

    async def _fetch():
        url = f"{_get_api_base()}/v1/me"
        headers = {"Authorization": f"Bearer {api_key}", "Accept": "application/json"}
        async with aiohttp.ClientSession() as session:
            timeout = aiohttp.ClientTimeout(total=10)
            async with session.get(url, headers=headers, timeout=timeout) as resp:
                try:
                    data = await resp.json(content_type=None)
                except Exception:
                    text = await resp.text()
                    return resp.status, {"error": text}
                return resp.status, data

    try:
        status_code, data = run_async(_fetch())
    except Exception as e:
        print_error(f"Failed to reach API: {e}")
        return 1

    if status_code == 401:
        clear_credentials()
        print_error("Session expired. Run 'cua auth login' to re-authenticate.")
        return 1

    if status_code != 200:
        print_error(f"Failed to fetch account info (HTTP {status_code})")
        return 1

    ws = data.get("workspace", {})
    org = data.get("organization", {})
    credits = data.get("credits", {})
    key_info = data.get("api_key", {})

    print_success("Logged in to cua.ai")
    print_info(f"  Workspace: {ws.get('name', 'unknown')} ({ws.get('slug', 'unknown')})")
    print_info(f"  Organization: {org.get('name', 'unknown')} ({org.get('plan_type', 'unknown')})")
    print_info(f"  Credits: {credits.get('balance', 0):.2f} remaining")
    print_info(f"  API key: {key_info.get('masked_value', 'unknown')}")

    if key_info.get("expires_at"):
        print_info(f"  Expires: {key_info['expires_at']}")

    return 0


def cmd_env(args: argparse.Namespace) -> int:
    """Handle the env command - export API key to .env file.

    Args:
        args: Parsed command-line arguments

    Returns:
        Exit code
    """
    api_key = get_api_key()
    if not api_key:
        print_error("Not authenticated. Run 'cua auth login' first.")
        return 1

    env_file = Path(args.file)
    env_line = f"CUA_API_KEY={api_key}"

    if env_file.exists():
        # Read existing content
        content = env_file.read_text()
        lines = content.splitlines()

        # Check if CUA_API_KEY already exists
        updated = False
        for i, line in enumerate(lines):
            if line.startswith("CUA_API_KEY="):
                lines[i] = env_line
                updated = True
                break

        if updated:
            env_file.write_text("\n".join(lines) + "\n")
            print_success(f"Updated CUA_API_KEY in {env_file}")
        else:
            # Append to file
            with env_file.open("a") as f:
                if content and not content.endswith("\n"):
                    f.write("\n")
                f.write(env_line + "\n")
            print_success(f"Added CUA_API_KEY to {env_file}")
    else:
        # Create new file
        env_file.write_text(env_line + "\n")
        print_success(f"Created {env_file} with CUA_API_KEY")

    return 0
