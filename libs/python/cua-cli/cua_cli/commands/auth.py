"""Authentication commands for Cua CLI."""

import argparse
import os
from pathlib import Path

import aiohttp
from core.http import cua_version_headers
from cua_cli.auth.browser import authenticate_via_browser
from cua_cli.auth.store import (
    ACTIVE_WORKSPACE_KEY,
    _get_store,
    clear_credentials,
    clear_legacy_credentials,
    delete_workspace,
    get_active_workspace,
    get_api_key,
    list_workspaces,
    save_api_key,
    save_workspace,
    set_active_workspace,
)
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
        description="Manage authentication for Cua cloud services",
    )

    auth_subparsers = auth_parser.add_subparsers(
        dest="auth_command",
        help="Authentication command",
    )

    # login command
    login_parser = auth_subparsers.add_parser(
        "login",
        help="Authenticate with Cua cloud",
        description="Authenticate via browser or API key",
    )
    login_parser.add_argument(
        "--api-key",
        type=str,
        help="API key for direct authentication (skips browser flow)",
    )

    # list command
    auth_subparsers.add_parser(
        "list",
        help="List authenticated workspaces",
        description="Show all authenticated workspaces",
    )

    # logout command
    logout_parser = auth_subparsers.add_parser(
        "logout",
        help="Clear stored credentials",
        description="Remove stored authentication credentials",
    )
    logout_parser.add_argument(
        "--workspace",
        type=str,
        help="Remove credentials for a specific workspace slug",
    )
    logout_parser.add_argument(
        "--all",
        action="store_true",
        help="Remove all workspace credentials",
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
    elif cmd == "list":
        return cmd_list(args)
    elif cmd == "env":
        return cmd_env(args)
    else:
        print_error("Usage: cua auth <command>")
        print_info("Commands: login, logout, status, list, env")
        return 1


def _fetch_me(api_key: str) -> tuple[int, dict]:
    """Call /v1/me to get workspace metadata for an API key."""

    async def _do():
        url = f"{_get_api_base()}/v1/me"
        headers = {
            "Authorization": f"Bearer {api_key}",
            "Accept": "application/json",
            **cua_version_headers(),
        }
        async with aiohttp.ClientSession() as session:
            timeout = aiohttp.ClientTimeout(total=10)
            async with session.get(url, headers=headers, timeout=timeout) as resp:
                try:
                    data = await resp.json(content_type=None)
                except Exception:
                    text = await resp.text()
                    return resp.status, {"error": text}
                return resp.status, data

    return run_async(_do())


def cmd_login(args: argparse.Namespace) -> int:
    """Handle the login command."""
    if args.api_key:
        # Direct API key authentication — discover workspace via /v1/me
        api_key = args.api_key
        print_info("Authenticating with provided API key...")
        try:
            status_code, data = _fetch_me(api_key)
        except Exception as e:
            print_error(f"Failed to reach API: {e}")
            return 1

        if status_code != 200:
            print_error(f"API key validation failed (HTTP {status_code})")
            return 1

        ws = data.get("workspace", {})
        org = data.get("organization", {})
        slug = ws.get("slug")
        if slug:
            name = ws.get("name", slug)
            org_name = org.get("name", "")
            save_workspace(slug, api_key, name, org_name)
            set_active_workspace(slug)
            print_success(f"Authenticated and switched to workspace: {name} ({slug})")
        else:
            # Legacy fallback — no workspace metadata from API
            save_api_key(api_key)
            _get_store().delete(ACTIVE_WORKSPACE_KEY)
            print_success("Successfully authenticated!")
    else:
        # Browser-based authentication
        try:
            result = run_async(authenticate_via_browser())
        except TimeoutError as e:
            print_error(str(e))
            return 1
        except RuntimeError as e:
            print_error(str(e))
            return 1

        if result.workspace_slug:
            save_workspace(
                result.workspace_slug,
                result.token,
                result.workspace_name,
                result.org_name,
            )
            set_active_workspace(result.workspace_slug)
            print_success(
                f"Authenticated and switched to workspace: "
                f"{result.workspace_name} ({result.workspace_slug})"
            )
        else:
            # Fallback if website didn't send workspace metadata
            save_api_key(result.token)
            _get_store().delete(ACTIVE_WORKSPACE_KEY)
            print_success("Successfully authenticated!")

    return 0


def cmd_logout(args: argparse.Namespace) -> int:
    """Handle the logout command."""
    if getattr(args, "all", False):
        clear_credentials()
        print_success("All credentials cleared.")
        return 0

    ws_slug = getattr(args, "workspace", None)
    if ws_slug:
        delete_workspace(ws_slug)
        active = get_active_workspace()
        if active == ws_slug:
            # Active workspace was removed — pick another or clear
            remaining = list_workspaces()
            if remaining:
                set_active_workspace(remaining[0]["slug"])
                print_info(
                    f"Switched to workspace: {remaining[0]['name']} ({remaining[0]['slug']})"
                )
            else:
                _get_store().delete(ACTIVE_WORKSPACE_KEY)
        print_success(f"Removed credentials for workspace: {ws_slug}")
        return 0

    # No flags — remove only the active workspace
    active = get_active_workspace()
    if active:
        delete_workspace(active)
        remaining = list_workspaces()
        if remaining:
            set_active_workspace(remaining[0]["slug"])
            print_success(f"Logged out of workspace: {active}")
            print_info(
                f"Switched to workspace: {remaining[0]['name']} ({remaining[0]['slug']}). "
                f"Use 'cua workspace set <slug>' to switch workspaces."
            )
        else:
            _get_store().delete(ACTIVE_WORKSPACE_KEY)
            print_success(f"Logged out of workspace: {active}")
    else:
        clear_legacy_credentials()
        print_success("Credentials cleared.")
    return 0


def cmd_list(args: argparse.Namespace) -> int:
    """Handle the list command — show all authenticated workspaces."""
    workspaces = list_workspaces()
    if not workspaces:
        print_info("No authenticated workspaces. Run 'cua auth login' to add one.")
        return 0

    # Group by org
    by_org: dict[str, list[dict]] = {}
    for ws in workspaces:
        org = ws["org"] or "Unknown"
        by_org.setdefault(org, []).append(ws)

    from rich.console import Console

    c = Console()
    for org, org_workspaces in by_org.items():
        c.print(f"[bold]{org}[/bold]")
        for ws in org_workspaces:
            name = ws["name"] or ws["slug"]
            slug = ws["slug"]
            if ws["is_active"]:
                c.print(f"  [green]* {name} ({slug})[/green]")
            else:
                c.print(f"    {name} ({slug})")
    return 0


def cmd_status(args: argparse.Namespace) -> int:
    """Handle the status command — show auth status and account info."""
    api_key = get_api_key()
    if not api_key:
        print_error("Not logged in. Run 'cua auth login' first.")
        return 1

    try:
        status_code, data = _fetch_me(api_key)
    except Exception as e:
        print_error(f"Failed to reach API: {e}")
        return 1

    if status_code == 401:
        # Don't mutate cached workspaces if the token came from the environment
        if os.environ.get("CUA_API_KEY"):
            print_error("Session expired. The CUA_API_KEY environment variable is invalid.")
            return 1

        active = get_active_workspace()
        if active:
            delete_workspace(active)
            print_info(f"Removed expired credentials for workspace: {active}")
            remaining = list_workspaces()
            if remaining:
                set_active_workspace(remaining[0]["slug"])
                print_info(
                    f"Switched to workspace: {remaining[0]['name']} ({remaining[0]['slug']})"
                )
            else:
                _get_store().delete(ACTIVE_WORKSPACE_KEY)
        else:
            clear_legacy_credentials()
            print_info("Removed expired credentials.")
        print_error("Session expired. Run 'cua auth login' to re-authenticate.")
        return 1

    if status_code != 200:
        print_error(f"Failed to fetch account info (HTTP {status_code})")
        return 1

    ws = data.get("workspace", {})
    org = data.get("organization", {})
    credits = data.get("credits", {})

    active_slug = get_active_workspace()
    active_label = f" [active: {active_slug}]" if active_slug else ""
    print_success(f"Logged in to cua.ai{active_label}")
    print_info(f"  Workspace: {ws.get('name', 'unknown')} ({ws.get('slug', 'unknown')})")
    print_info(f"  Organization: {org.get('name', 'unknown')} ({org.get('plan_type', 'unknown')})")
    print_info(f"  Credits: {credits.get('balance', 0):.2f} remaining")

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
