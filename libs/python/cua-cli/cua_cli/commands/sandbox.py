"""Sandbox management commands for CUA CLI."""

import argparse
import os
import webbrowser
from typing import Any, Optional
from urllib.parse import quote

import aiohttp
from core.http import cua_version_headers
from cua_cli.auth.store import require_api_key
from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import (
    print_error,
    print_info,
    print_json,
    print_success,
    print_table,
)

DEFAULT_API_BASE = "https://api.cua.ai"


def _get_api_base() -> str:
    """Get the API base URL."""
    return os.environ.get("CUA_API_BASE", DEFAULT_API_BASE).rstrip("/")


async def _api_request(
    method: str,
    path: str,
    api_key: str,
    json: Optional[dict] = None,
    timeout: int = 30,
) -> tuple[int, Any]:
    """Make an HTTP request to the CUA API."""
    url = f"{_get_api_base()}{path}"
    headers = {
        "Authorization": f"Bearer {api_key}",
        "Accept": "application/json",
        **cua_version_headers(),
    }

    if json is not None:
        headers["Content-Type"] = "application/json"

    async with aiohttp.ClientSession() as session:
        timeout_obj = aiohttp.ClientTimeout(total=timeout)
        async with session.request(
            method, url, headers=headers, json=json, timeout=timeout_obj
        ) as resp:
            try:
                data = await resp.json(content_type=None)
            except Exception:
                data = await resp.text()
            return resp.status, data


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the sandbox command and subcommands."""
    # Register both 'sandbox' and 'sb' as aliases
    for cmd_name in ("sandbox", "sb"):
        sb_parser = subparsers.add_parser(
            cmd_name,
            help="Sandbox management commands",
            description="Manage cloud sandboxes (virtual machines)",
        )

        sb_subparsers = sb_parser.add_subparsers(
            dest="sandbox_command",
            help="Sandbox command",
        )

        # list command
        list_parser = sb_subparsers.add_parser(
            "list",
            aliases=["ls", "ps"],
            help="List all sandboxes",
        )
        list_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )
        list_parser.add_argument(
            "--show-passwords",
            action="store_true",
            help="Show sandbox passwords in output",
        )

        # create command
        create_parser = sb_subparsers.add_parser(
            "create",
            help="Create a new sandbox",
        )
        create_parser.add_argument(
            "--os",
            required=True,
            choices=["linux", "windows", "macos"],
            help="Operating system",
        )
        create_parser.add_argument(
            "--size",
            required=True,
            choices=["small", "medium", "large"],
            help="Sandbox size",
        )
        create_parser.add_argument(
            "--region",
            required=True,
            choices=["north-america", "europe", "asia-pacific", "south-america"],
            help="Region for the sandbox",
        )
        create_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )

        # get command
        get_parser = sb_subparsers.add_parser(
            "get",
            help="Get sandbox details",
        )
        get_parser.add_argument(
            "name",
            help="Sandbox name",
        )
        get_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )
        get_parser.add_argument(
            "--show-passwords",
            action="store_true",
            help="Show sandbox password",
        )
        get_parser.add_argument(
            "--show-vnc-url",
            action="store_true",
            help="Show VNC URL",
        )

        # delete command
        delete_parser = sb_subparsers.add_parser(
            "delete",
            help="Delete a sandbox",
        )
        delete_parser.add_argument(
            "name",
            help="Sandbox name",
        )

        # start command
        start_parser = sb_subparsers.add_parser(
            "start",
            help="Start a stopped sandbox",
        )
        start_parser.add_argument(
            "name",
            help="Sandbox name",
        )

        # stop command
        stop_parser = sb_subparsers.add_parser(
            "stop",
            help="Stop a running sandbox",
        )
        stop_parser.add_argument(
            "name",
            help="Sandbox name",
        )

        # restart command
        restart_parser = sb_subparsers.add_parser(
            "restart",
            help="Restart a sandbox",
        )
        restart_parser.add_argument(
            "name",
            help="Sandbox name",
        )

        # suspend command
        suspend_parser = sb_subparsers.add_parser(
            "suspend",
            help="Suspend a sandbox (preserves memory state)",
        )
        suspend_parser.add_argument(
            "name",
            help="Sandbox name",
        )

        # vnc command
        vnc_parser = sb_subparsers.add_parser(
            "vnc",
            aliases=["open"],
            help="Open sandbox in browser (VNC)",
        )
        vnc_parser.add_argument(
            "name",
            help="Sandbox name",
        )


def execute(args: argparse.Namespace) -> int:
    """Execute sandbox command based on subcommand."""
    cmd = getattr(args, "sandbox_command", None)

    if cmd in ("list", "ls", "ps"):
        return cmd_list(args)
    elif cmd == "create":
        return cmd_create(args)
    elif cmd == "get":
        return cmd_get(args)
    elif cmd == "delete":
        return cmd_delete(args)
    elif cmd == "start":
        return cmd_start(args)
    elif cmd == "stop":
        return cmd_stop(args)
    elif cmd == "restart":
        return cmd_restart(args)
    elif cmd == "suspend":
        return cmd_suspend(args)
    elif cmd in ("vnc", "open"):
        return cmd_vnc(args)
    else:
        print_error("Usage: cua sandbox <command>")
        print_info("Commands: list, create, get, delete, start, stop, restart, suspend, vnc")
        return 1


def _get_provider():
    """Get a configured CloudProvider instance."""
    from computer.providers.cloud.provider import CloudProvider

    api_key = require_api_key()
    return CloudProvider(api_key=api_key)


def _redact_sensitive(vm: dict) -> dict:
    """Remove password and VNC URL from a VM dict."""
    redacted = {k: v for k, v in vm.items() if k not in ("password", "vnc_url")}
    return redacted


def cmd_list(args: argparse.Namespace) -> int:
    """List all sandboxes."""

    async def _list():
        async with _get_provider() as provider:
            return await provider.list_vms()

    vms = run_async(_list())

    if args.json:
        data = vms if args.show_passwords else [_redact_sensitive(vm) for vm in vms]
        print_json(data)
        return 0

    if not vms:
        print_info("No sandboxes found.")
        return 0

    # Format for table display
    columns = [
        ("name", "NAME"),
        ("status", "STATUS"),
        ("host", "HOST"),
    ]

    if args.show_passwords:
        columns.append(("password", "PASSWORD"))

    print_table(vms, columns)
    return 0


def cmd_create(args: argparse.Namespace) -> int:
    """Create a new sandbox."""
    api_key = require_api_key()

    async def _create():
        body = {
            "os": args.os,
            "configuration": args.size,
            "region": args.region,
        }
        status_code, data = await _api_request("POST", "/v1/vms", api_key, json=body)

        if status_code == 200:
            # Sandbox ready immediately
            return {
                "status": data.get("status", "ready") if isinstance(data, dict) else "ready",
                "name": data.get("name") if isinstance(data, dict) else None,
                "password": data.get("password") if isinstance(data, dict) else None,
                "host": data.get("host") if isinstance(data, dict) else None,
            }
        elif status_code == 202:
            # Provisioning in progress
            return {
                "status": (
                    data.get("status", "provisioning") if isinstance(data, dict) else "provisioning"
                ),
                "name": data.get("name") if isinstance(data, dict) else None,
                "job_id": data.get("job_id") if isinstance(data, dict) else None,
            }
        elif status_code == 401:
            return {"status": "unauthorized"}
        elif status_code == 400:
            return {"status": "invalid_request", "message": str(data)}
        else:
            return {"status": "error", "message": str(data)}

    result = run_async(_create())

    if args.json:
        print_json(result)
        return 0 if result.get("status") not in ("error", "unauthorized", "invalid_request") else 1

    status = result.get("status")

    if status in ("error", "unauthorized", "invalid_request"):
        print_error(f"Failed to create sandbox: {result.get('message', status)}")
        return 1

    if status == "provisioning":
        print_info(f"Sandbox '{result.get('name')}' is being provisioned...")
        print_info("Use 'cua sb list' to check status.")
    else:
        print_success(f"Sandbox '{result.get('name')}' created!")
        if result.get("password"):
            print_info(f"Password: {result.get('password')}")
        if result.get("host"):
            print_info(f"Host: {result.get('host')}")

    return 0


def cmd_get(args: argparse.Namespace) -> int:
    """Get sandbox details."""

    async def _get():
        async with _get_provider() as provider:
            return await provider.get_vm(args.name)

    result = run_async(_get())

    if args.json:
        data = result if args.show_passwords else _redact_sensitive(result)
        print_json(data)
        return 1 if result.get("status") == "not_found" else 0

    if result.get("status") == "not_found":
        print_error(f"Sandbox '{args.name}' not found.")
        return 1

    # Display sandbox info
    print_info(f"Name: {result.get('name')}")
    print_info(f"Status: {result.get('status')}")

    if result.get("os_type"):
        print_info(f"OS: {result.get('os_type')}")
    if result.get("host"):
        print_info(f"Host: {result.get('host')}")
    if args.show_passwords and result.get("password"):
        print_info(f"Password: {result.get('password')}")
    if args.show_vnc_url and result.get("vnc_url"):
        print_info(f"VNC URL: {result.get('vnc_url')}")

    return 0


def cmd_delete(args: argparse.Namespace) -> int:
    """Delete a sandbox."""
    api_key = require_api_key()

    async def _delete():
        status_code, data = await _api_request("DELETE", f"/v1/vms/{args.name}", api_key)

        if status_code in (200, 202, 204):
            body_status = data.get("status") if isinstance(data, dict) else None
            return {"name": args.name, "status": body_status or "deleting"}
        elif status_code == 404:
            return {"name": args.name, "status": "not_found"}
        elif status_code == 401:
            return {"name": args.name, "status": "unauthorized"}
        else:
            return {"name": args.name, "status": "error", "message": str(data)}

    result = run_async(_delete())
    status = result.get("status")

    if status == "not_found":
        print_error(f"Sandbox '{args.name}' not found.")
        return 1
    elif status in ("error", "unauthorized"):
        print_error(f"Failed to delete sandbox: {result.get('message', status)}")
        return 1
    else:
        print_success(f"Sandbox '{args.name}' is being deleted.")
        return 0


def cmd_start(args: argparse.Namespace) -> int:
    """Start a stopped sandbox."""

    async def _start():
        async with _get_provider() as provider:
            return await provider.run_vm(name=args.name)

    result = run_async(_start())
    status = result.get("status")

    if status == "not_found":
        print_error(f"Sandbox '{args.name}' not found.")
        return 1
    elif status in ("error", "unauthorized"):
        print_error(f"Failed to start sandbox: {result.get('message', status)}")
        return 1
    else:
        print_success(f"Sandbox '{args.name}' is starting.")
        return 0


def cmd_stop(args: argparse.Namespace) -> int:
    """Stop a running sandbox."""

    async def _stop():
        async with _get_provider() as provider:
            return await provider.stop_vm(args.name)

    result = run_async(_stop())
    status = result.get("status")

    if status == "not_found":
        print_error(f"Sandbox '{args.name}' not found.")
        return 1
    elif status in ("error", "unauthorized"):
        print_error(f"Failed to stop sandbox: {result.get('message', status)}")
        return 1
    else:
        print_success(f"Sandbox '{args.name}' is stopping.")
        return 0


def cmd_restart(args: argparse.Namespace) -> int:
    """Restart a sandbox."""

    async def _restart():
        async with _get_provider() as provider:
            return await provider.restart_vm(args.name)

    result = run_async(_restart())
    status = result.get("status")

    if status == "not_found":
        print_error(f"Sandbox '{args.name}' not found.")
        return 1
    elif status in ("error", "unauthorized"):
        print_error(f"Failed to restart sandbox: {result.get('message', status)}")
        return 1
    else:
        print_success(f"Sandbox '{args.name}' is restarting.")
        return 0


def cmd_suspend(args: argparse.Namespace) -> int:
    """Suspend a sandbox."""
    api_key = require_api_key()

    async def _suspend():
        status_code, data = await _api_request("POST", f"/v1/vms/{args.name}/suspend", api_key)

        if status_code in (200, 202):
            body_status = data.get("status") if isinstance(data, dict) else None
            return {"name": args.name, "status": body_status or "suspending"}
        elif status_code == 404:
            return {"name": args.name, "status": "not_found"}
        elif status_code == 401:
            return {"name": args.name, "status": "unauthorized"}
        elif status_code == 400:
            # Suspend may not be supported for all VM types
            return {"name": args.name, "status": "unsupported", "message": str(data)}
        else:
            return {"name": args.name, "status": "error", "message": str(data)}

    result = run_async(_suspend())
    status = result.get("status")

    if status == "not_found":
        print_error(f"Sandbox '{args.name}' not found.")
        return 1
    elif status == "unsupported":
        print_error(f"Suspend not supported for this sandbox: {result.get('message', '')}")
        return 1
    elif status in ("error", "unauthorized"):
        print_error(f"Failed to suspend sandbox: {result.get('message', status)}")
        return 1
    else:
        print_success(f"Sandbox '{args.name}' is suspending.")
        return 0


def cmd_vnc(args: argparse.Namespace) -> int:
    """Open sandbox in browser via VNC."""

    async def _get_vnc_url():
        async with _get_provider() as provider:
            vms = await provider.list_vms()
            vm_info = next((vm for vm in vms if vm.get("name") == args.name), None)
            return vm_info

    vm_info = run_async(_get_vnc_url())

    if not vm_info:
        print_error(f"Sandbox '{args.name}' not found.")
        return 1

    # Always construct VNC URL from host (the API's vnc_url may use a stale domain)
    host = vm_info.get("host")
    password = vm_info.get("password")

    if host and password:
        encoded_password = quote(password, safe="")
        vnc_url = (
            f"https://{host}/vnc.html?autoconnect=true&password={encoded_password}&show_dot=true"
        )
    else:
        print_error("Could not determine VNC URL. Sandbox may not be ready.")
        return 1

    print_info(f"Opening VNC: {vnc_url}")
    webbrowser.open(vnc_url)
    return 0
