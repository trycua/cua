"""Workspace commands for Cua CLI."""

import argparse
import os

import aiohttp
from core.http import cua_version_headers
from cua_cli.auth.browser import authenticate_via_browser
from cua_cli.auth.store import (
    _get_store,
    delete_workspace,
    get_workspace_api_key,
    list_workspaces,
    save_workspace,
    set_active_workspace,
)
from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import print_error, print_info, print_success

DEFAULT_API_BASE = "https://api.cua.ai"


def _get_api_base() -> str:
    return os.environ.get("CUA_API_BASE", DEFAULT_API_BASE).rstrip("/")


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the workspace command and subcommands."""
    ws_parser = subparsers.add_parser(
        "workspace",
        help="Workspace commands",
        description="Manage workspaces",
        aliases=["ws"],
    )

    ws_subparsers = ws_parser.add_subparsers(
        dest="workspace_command",
        help="Workspace command",
    )

    set_parser = ws_subparsers.add_parser(
        "set",
        help="Switch active workspace",
        description="Switch to a different workspace",
    )
    set_parser.add_argument(
        "slug",
        nargs="?",
        help="Workspace slug to switch to",
    )


def execute(args: argparse.Namespace) -> int:
    """Execute workspace command based on subcommand."""
    cmd = getattr(args, "workspace_command", None)

    if cmd == "set":
        return cmd_set(args)
    else:
        print_error("Usage: cua workspace <command>")
        print_info("Commands: set")
        return 1


def _validate_workspace_key(api_key: str) -> tuple[bool, dict]:
    """Validate an API key by calling /v1/me. Returns (valid, data)."""

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
                    return resp.status, {}
                return resp.status, data

    try:
        status_code, data = run_async(_do())
        return status_code == 200, data
    except Exception:
        return False, {}


def cmd_set(args: argparse.Namespace) -> int:
    """Handle the workspace set command."""
    slug = getattr(args, "slug", None)

    if slug:
        # Check if we have a stored key for this workspace
        stored_key = get_workspace_api_key(slug)
        if stored_key:
            # Validate the stored key
            valid, _data = _validate_workspace_key(stored_key)
            if valid:
                set_active_workspace(slug)
                ws_name = _get_store().get(f"workspace:{slug}:name") or slug
                print_success(f"Switched to workspace: {ws_name} ({slug})")
                return 0
            else:
                # Stale key — remove and fall through to browser auth
                print_info(f"Credentials for '{slug}' are expired. Re-authenticating...")
                delete_workspace(slug)

        # Not authenticated or stale — trigger browser auth with hint
        try:
            result = run_async(authenticate_via_browser(workspace_slug=slug))
        except (TimeoutError, RuntimeError) as e:
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
            print_error("Authentication did not return workspace metadata.")
            return 1
        return 0

    # No slug — interactive selection from authenticated workspaces
    workspaces = list_workspaces()
    if not workspaces:
        print_info("No authenticated workspaces. Run 'cua auth login' to add one.")
        return 0

    print_info("Select a workspace:")
    for i, ws in enumerate(workspaces, 1):
        marker = " (active)" if ws["is_active"] else ""
        name_part = f" - {ws['name']}" if ws["name"] else ""
        print(f"  {i}. {ws['slug']}{name_part}{marker}")

    try:
        choice = input("\nEnter number: ").strip()
        idx = int(choice) - 1
        if idx < 0 or idx >= len(workspaces):
            print_error("Invalid selection.")
            return 1
    except (ValueError, EOFError, KeyboardInterrupt):
        print_error("Invalid selection.")
        return 1

    selected = workspaces[idx]
    set_active_workspace(selected["slug"])
    print_success(f"Switched to workspace: {selected['name']} ({selected['slug']})")
    return 0
