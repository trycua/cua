"""Sandbox management commands for CUA CLI."""

import argparse
import asyncio
import base64
import json
import os
import shutil
import sys
import webbrowser
from typing import Any, Optional
from urllib.parse import quote

import aiohttp
from core.http import cua_version_headers
from cua_cli.auth.store import get_api_key, require_api_key
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

        # shell command
        # Note: Options must come before 'name' due to argparse REMAINDER behavior
        # Usage: cua sb shell [--cols N] [--rows N] <name> [command...]
        shell_parser = sb_subparsers.add_parser(
            "shell",
            help="Open interactive shell or run command in sandbox",
        )
        shell_parser.add_argument(
            "--cols",
            type=int,
            default=None,
            help="Terminal width (default: auto-detect)",
        )
        shell_parser.add_argument(
            "--rows",
            type=int,
            default=None,
            help="Terminal height (default: auto-detect)",
        )
        shell_parser.add_argument(
            "name",
            help="Sandbox name",
        )
        shell_parser.add_argument(
            "shell_command",
            nargs=argparse.REMAINDER,
            help="Command to run (optional, opens interactive shell if omitted)",
        )

        # exec command
        # Note: --json must come before 'name' due to argparse REMAINDER behavior
        # Usage: cua sb exec [--json] <name> <command...>
        exec_parser = sb_subparsers.add_parser(
            "exec",
            help="Execute command in sandbox (non-interactive)",
        )
        exec_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )
        exec_parser.add_argument(
            "name",
            help="Sandbox name",
        )
        exec_parser.add_argument(
            "exec_command",
            nargs=argparse.REMAINDER,
            help="Command to execute",
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
    elif cmd == "shell":
        return cmd_shell(args)
    elif cmd == "exec":
        return cmd_exec(args)
    else:
        print_error("Usage: cua sandbox <command>")
        print_info(
            "Commands: list, create, get, delete, start, stop, restart, suspend, vnc, shell, exec"
        )
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


def _default_shell() -> str:
    """Get the default shell for the platform."""
    return "powershell" if sys.platform == "win32" else "bash"


async def _get_sandbox_api_url(name: str) -> tuple[str, str]:
    """Get the API URL and api_key for a sandbox.

    Returns:
        Tuple of (api_url, api_key)

    Raises:
        ValueError: If sandbox not found or not ready
    """
    api_key = require_api_key()
    async with _get_provider() as provider:
        vm = await provider.get_vm(name)
        if not vm:
            raise ValueError(f"Sandbox '{name}' not found")
        if vm.get("status") == "not_found":
            raise ValueError(f"Sandbox '{name}' not found")
        api_url = vm.get("api_url")
        if not api_url:
            raise ValueError(f"Sandbox '{name}' has no API URL (is it running?)")
        return api_url, api_key


async def _shell_interactive(
    name: str,
    api_url: str,
    api_key: str,
    command: Optional[str],
    cols: Optional[int],
    rows: Optional[int],
) -> int:
    """Run an interactive PTY session via WebSocket."""
    import signal
    import threading

    _auto_cols, _auto_rows = shutil.get_terminal_size((80, 24))
    cols = cols if cols is not None else _auto_cols
    rows = rows if rows is not None else _auto_rows

    ws_url = api_url.replace("https://", "wss://").replace("http://", "ws://")

    headers = {
        "X-API-Key": api_key,
        "X-Container-Name": name,
        "Content-Type": "application/json",
    }
    ws_params = {
        "api_key": api_key,
        "container_name": name,
    }

    # Create PTY session
    try:
        async with aiohttp.ClientSession() as http:
            async with http.post(
                f"{api_url}/pty",
                json={"command": command or _default_shell(), "cols": cols, "rows": rows},
                headers=headers,
                timeout=aiohttp.ClientTimeout(total=15),
            ) as resp:
                if resp.status == 401:
                    raise ValueError("Authentication failed. Is the sandbox running?")
                resp.raise_for_status()
                data = await resp.json()
    except aiohttp.ClientResponseError as e:
        raise ValueError(f"PTY unavailable: {e.status} {e.message}") from e
    except aiohttp.ClientError as e:
        raise ValueError(f"Connection failed: {e}") from e

    pid: int = data["pid"]
    exit_code_cell: list[int] = [0]
    done_event = asyncio.Event()

    if sys.platform == "win32":
        import msvcrt

        async def _run_ws() -> None:
            async with aiohttp.ClientSession() as http:
                async with http.ws_connect(f"{ws_url}/pty/{pid}/ws", params=ws_params) as ws:

                    def _stdin_loop() -> None:
                        while not done_event.is_set():
                            try:
                                ch = msvcrt.getch()
                                if ch:
                                    encoded = base64.b64encode(ch).decode()
                                    asyncio.run_coroutine_threadsafe(
                                        ws.send_str(json.dumps({"type": "stdin", "data": encoded})),
                                        asyncio.get_event_loop(),
                                    )
                            except Exception:
                                break

                    t = threading.Thread(target=_stdin_loop, daemon=True)
                    t.start()

                    async for raw_msg in ws:
                        if raw_msg.type == aiohttp.WSMsgType.TEXT:
                            try:
                                msg = json.loads(raw_msg.data)
                            except Exception:
                                continue
                            if msg.get("type") == "output":
                                chunk = base64.b64decode(msg["data"])
                                sys.stdout.buffer.write(chunk)
                                sys.stdout.buffer.flush()
                            elif msg.get("type") == "exit":
                                exit_code_cell[0] = int(msg.get("code", 0))
                                break
                        elif raw_msg.type in (aiohttp.WSMsgType.CLOSED, aiohttp.WSMsgType.ERROR):
                            break
                    done_event.set()

        await _run_ws()
    else:
        import termios
        import tty

        old_settings = termios.tcgetattr(sys.stdin.fileno())
        tty.setraw(sys.stdin.fileno())

        loop = asyncio.get_event_loop()

        async def _run_ws() -> None:
            async with aiohttp.ClientSession() as http:
                async with http.ws_connect(f"{ws_url}/pty/{pid}/ws", params=ws_params) as ws:

                    def _resize(_sig=None, _frame=None) -> None:
                        c, r = shutil.get_terminal_size((80, 24))
                        asyncio.run_coroutine_threadsafe(
                            ws.send_str(json.dumps({"type": "resize", "cols": c, "rows": r})),
                            loop,
                        )

                    signal.signal(signal.SIGWINCH, _resize)

                    def _stdin_loop() -> None:
                        while not done_event.is_set():
                            try:
                                ch = sys.stdin.buffer.read(1)
                                if not ch:
                                    break
                                encoded = base64.b64encode(ch).decode()
                                asyncio.run_coroutine_threadsafe(
                                    ws.send_str(json.dumps({"type": "stdin", "data": encoded})),
                                    loop,
                                )
                            except Exception:
                                break

                    t = threading.Thread(target=_stdin_loop, daemon=True)
                    t.start()

                    async for raw_msg in ws:
                        if raw_msg.type == aiohttp.WSMsgType.TEXT:
                            try:
                                msg = json.loads(raw_msg.data)
                            except Exception:
                                continue
                            if msg.get("type") == "output":
                                chunk = base64.b64decode(msg["data"])
                                sys.stdout.buffer.write(chunk)
                                sys.stdout.buffer.flush()
                            elif msg.get("type") == "exit":
                                exit_code_cell[0] = int(msg.get("code", 0))
                                break
                        elif raw_msg.type in (aiohttp.WSMsgType.CLOSED, aiohttp.WSMsgType.ERROR):
                            break
                    done_event.set()

        try:
            await _run_ws()
        finally:
            try:
                termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, old_settings)
            except Exception:
                pass
            signal.signal(signal.SIGWINCH, signal.SIG_DFL)

    return exit_code_cell[0]


async def _exec_noninteractive(
    name: str,
    api_url: str,
    api_key: str,
    command: str,
) -> dict:
    """Execute a command non-interactively and return result."""
    headers = {
        "X-API-Key": api_key,
        "X-Container-Name": name,
        "Content-Type": "application/json",
    }

    # Use the /cmd endpoint with run_command for non-interactive execution
    async with aiohttp.ClientSession() as http:
        async with http.post(
            f"{api_url}/cmd",
            json={"command": "run_command", "params": {"command": command}},
            headers=headers,
            timeout=aiohttp.ClientTimeout(total=120),
        ) as resp:
            if resp.status == 401:
                return {"success": False, "error": "Authentication failed"}
            text = await resp.text()
            # Parse SSE-style response
            for line in text.splitlines():
                if line.startswith("data: "):
                    try:
                        return json.loads(line[6:])
                    except json.JSONDecodeError:
                        pass
            # Try parsing as plain JSON
            try:
                return json.loads(text)
            except json.JSONDecodeError:
                return {"success": False, "error": f"Unexpected response: {text[:200]}"}


def cmd_shell(args: argparse.Namespace) -> int:
    """Open interactive shell or run command in sandbox."""
    command_parts = getattr(args, "shell_command", [])
    command = " ".join(command_parts).strip() if command_parts else None

    cols: Optional[int] = getattr(args, "cols", None)
    rows: Optional[int] = getattr(args, "rows", None)

    async def _run() -> int:
        try:
            api_url, api_key = await _get_sandbox_api_url(args.name)
        except ValueError as e:
            print_error(str(e))
            return 1

        # Non-interactive mode when stdin is not a TTY
        if not sys.stdin.isatty():
            if not command:
                print_error("No command provided for non-interactive mode")
                return 1
            result = await _exec_noninteractive(args.name, api_url, api_key, command)
            if not result.get("success", True):
                print_error(result.get("error", "Command failed"))
                return 1
            stdout = result.get("stdout", "").strip()
            stderr = result.get("stderr", "").strip()
            returncode = result.get("returncode") or result.get("return_code") or 0
            if stdout:
                print(stdout)
            if stderr:
                print(stderr, file=sys.stderr)
            return returncode

        # Interactive mode
        try:
            return await _shell_interactive(args.name, api_url, api_key, command, cols, rows)
        except ValueError as e:
            print_error(str(e))
            return 1

    return run_async(_run())


def cmd_exec(args: argparse.Namespace) -> int:
    """Execute command in sandbox (non-interactive)."""
    command_parts = getattr(args, "exec_command", [])
    command = " ".join(command_parts).strip() if command_parts else None

    if not command:
        print_error("No command provided")
        print_info("Usage: cua sb exec <name> <command>")
        return 1

    async def _run() -> int:
        try:
            api_url, api_key = await _get_sandbox_api_url(args.name)
        except ValueError as e:
            print_error(str(e))
            return 1

        result = await _exec_noninteractive(args.name, api_url, api_key, command)

        if args.json:
            print_json(result)
            # Return the actual exit code from the command
            return result.get("returncode") or result.get("return_code") or 0

        if not result.get("success", True):
            print_error(result.get("error", "Command failed"))
            return 1

        stdout = result.get("stdout", "").strip()
        stderr = result.get("stderr", "").strip()
        returncode = result.get("returncode") or result.get("return_code") or 0

        if stdout:
            print(stdout)
        if stderr:
            print(stderr, file=sys.stderr)

        return returncode

    return run_async(_run())
