"""Sandbox management commands for CUA CLI."""

import argparse
import asyncio
import base64
import json
import shutil
import sys
import webbrowser
from typing import Any, Optional

import aiohttp
from cua_cli.auth.store import get_api_key, require_api_key
from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import (
    print_error,
    print_info,
    print_json,
    print_success,
    print_table,
)

# ---------------------------------------------------------------------------
# Image string parsing
# ---------------------------------------------------------------------------

_REGISTRY_HOSTNAMES = {
    "ghcr.io",
    "docker.io",
    "registry.hub.docker.com",
    "quay.io",
    "gcr.io",
    "registry.cua.ai",
}

_MACOS_ALIASES = {"macos", "mac", "osx"}
_LINUX_ALIASES = {"linux", "ubuntu", "debian", "fedora"}
_WINDOWS_ALIASES = {"windows", "win"}
_ANDROID_ALIASES = {"android"}


def _parse_image(image_str: str, vm: bool = False):
    """Parse an image string into a cua_sandbox Image object.

    Examples::

        "macos"           -> Image.macos("26")
        "macos:sequoia"   -> Image.macos("sequoia")
        "ubuntu:24.04"    -> Image.linux("ubuntu", "24.04")
        "linux"           -> Image.linux("ubuntu", "24.04")
        "windows:11"      -> Image.windows("11")
        "android:14"      -> Image.android("14")
        "ghcr.io/org/img" -> Image.from_registry("ghcr.io/org/img")
    """
    from cua_sandbox import Image

    # Registry reference: contains '/' or starts with a known registry hostname
    if "/" in image_str:
        host = image_str.split("/")[0]
        if "." in host or host in _REGISTRY_HOSTNAMES:
            return Image.from_registry(image_str)
    for rh in _REGISTRY_HOSTNAMES:
        if image_str.startswith(rh):
            return Image.from_registry(image_str)

    # Split on ':'
    parts = image_str.split(":", 1)
    base = parts[0].lower()
    tag = parts[1] if len(parts) > 1 else None

    if base in _MACOS_ALIASES:
        version = tag or "26"
        return Image.macos(version)

    if base in _LINUX_ALIASES:
        distro = base if base != "linux" else "ubuntu"
        version = tag or "24.04"
        kind = "vm" if vm else "container"
        return Image.linux(distro, version, kind=kind)

    if base in _WINDOWS_ALIASES:
        version = tag or "11"
        return Image.windows(version)

    if base in _ANDROID_ALIASES:
        version = tag or "14"
        return Image.android(version)

    # Unknown — try registry
    return Image.from_registry(image_str)


# ---------------------------------------------------------------------------
# Memory / disk parsing
# ---------------------------------------------------------------------------


def _parse_memory(s: str) -> int:
    """Convert a memory string to MB.

    "8GB"    -> 8192
    "8192MB" -> 8192
    "8"      -> 8192  (bare number treated as GB)
    """
    s = s.strip()
    if s.upper().endswith("GB"):
        return int(float(s[:-2])) * 1024
    if s.upper().endswith("MB"):
        return int(float(s[:-2]))
    # Bare number — treat as GB
    return int(float(s)) * 1024


def _parse_disk(s: str) -> int:
    """Convert a disk string to GB.

    "50GB" -> 50
    "50"   -> 50
    "51200MB" -> 50
    """
    s = s.strip()
    if s.upper().endswith("GB"):
        return int(float(s[:-2]))
    if s.upper().endswith("MB"):
        return int(float(s[:-2])) // 1024
    return int(float(s))


# ---------------------------------------------------------------------------
# Sandbox API URL resolution (for shell / exec)
# ---------------------------------------------------------------------------


async def _get_sandbox_api_url(name: str, local: bool) -> tuple[str, Optional[str]]:
    """Resolve the computer-server API URL and optional api_key for a sandbox.

    Returns:
        Tuple of (api_url, api_key_or_none)
    """
    if local:
        from cua_sandbox import sandbox_state

        state = sandbox_state.load(name)
        if not state:
            raise ValueError(f"Local sandbox '{name}' not found. Check ~/.cua/sandboxes/")
        url = f"http://{state['host']}:{state['api_port']}"
        return url, None

    api_key = require_api_key()
    from cua_sandbox.transport.cloud import CloudTransport, cloud_get_vm

    vm = await cloud_get_vm(name, api_key=api_key)
    if not vm:
        raise ValueError(f"Sandbox '{name}' not found")
    if vm.get("status") == "not_found":
        raise ValueError(f"Sandbox '{name}' not found")
    url = CloudTransport._resolve_endpoint(vm)
    if not url:
        raise ValueError(f"Sandbox '{name}' has no API URL (is it running?)")
    return url, api_key


# ---------------------------------------------------------------------------
# Parser registration
# ---------------------------------------------------------------------------


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the sandbox command and subcommands."""
    for cmd_name in ("sandbox", "sb"):
        sb_parser = subparsers.add_parser(
            cmd_name,
            help="Sandbox management commands",
            description="Manage cloud and local sandboxes",
        )

        sb_subparsers = sb_parser.add_subparsers(
            dest="sandbox_command",
            help="Sandbox command",
        )

        # -- launch ----------------------------------------------------------
        launch_parser = sb_subparsers.add_parser(
            "launch",
            help="Launch a new sandbox",
        )
        launch_parser.add_argument(
            "image",
            help="Image to launch (e.g. macos, ubuntu:24.04, windows:11)",
        )
        launch_parser.add_argument(
            "--local",
            action="store_true",
            help="Launch a local sandbox",
        )
        launch_parser.add_argument(
            "--name",
            default=None,
            help="Sandbox name",
        )
        launch_parser.add_argument(
            "--vm",
            action="store_true",
            help="Force VM kind for Linux images (default: container)",
        )
        launch_parser.add_argument(
            "--cpu",
            type=int,
            default=None,
            help="Number of vCPUs",
        )
        launch_parser.add_argument(
            "--memory",
            default=None,
            help="Memory (e.g. 8GB, 4096MB)",
        )
        launch_parser.add_argument(
            "--disk",
            default=None,
            help="Disk size (e.g. 50GB)",
        )
        launch_parser.add_argument(
            "--region",
            default=None,
            help="Cloud region",
        )
        launch_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )

        # -- ls / list -------------------------------------------------------
        ls_parser = sb_subparsers.add_parser(
            "ls",
            aliases=["list"],
            help="List sandboxes",
        )
        ls_parser.add_argument(
            "--local",
            action="store_true",
            help="List local sandboxes",
        )
        ls_parser.add_argument(
            "--all",
            action="store_true",
            help="List both local and cloud sandboxes",
        )
        ls_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )

        # -- info ------------------------------------------------------------
        info_parser = sb_subparsers.add_parser(
            "info",
            aliases=["get"],
            help="Get sandbox details",
        )
        info_parser.add_argument(
            "name",
            help="Sandbox name",
        )
        info_parser.add_argument(
            "--local",
            action="store_true",
            help="Target a local sandbox",
        )
        info_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )

        # -- suspend ---------------------------------------------------------
        suspend_parser = sb_subparsers.add_parser(
            "suspend",
            help="Suspend a sandbox (preserves memory state)",
        )
        suspend_parser.add_argument("name", help="Sandbox name")
        suspend_parser.add_argument("--local", action="store_true", help="Target a local sandbox")

        # -- resume ----------------------------------------------------------
        resume_parser = sb_subparsers.add_parser(
            "resume",
            help="Resume a suspended sandbox",
        )
        resume_parser.add_argument("name", help="Sandbox name")
        resume_parser.add_argument("--local", action="store_true", help="Target a local sandbox")

        # -- restart ---------------------------------------------------------
        restart_parser = sb_subparsers.add_parser(
            "restart",
            help="Restart a sandbox",
        )
        restart_parser.add_argument("name", help="Sandbox name")
        restart_parser.add_argument("--local", action="store_true", help="Target a local sandbox")

        # -- delete ----------------------------------------------------------
        delete_parser = sb_subparsers.add_parser(
            "delete",
            help="Delete a sandbox",
        )
        delete_parser.add_argument("name", help="Sandbox name")
        delete_parser.add_argument("--local", action="store_true", help="Target a local sandbox")
        delete_parser.add_argument(
            "--force",
            action="store_true",
            help="Skip confirmation prompt",
        )

        # -- vnc -------------------------------------------------------------
        vnc_parser = sb_subparsers.add_parser(
            "vnc",
            help="Open sandbox in browser via VNC",
        )
        vnc_parser.add_argument("name", help="Sandbox name")
        vnc_parser.add_argument("--local", action="store_true", help="Target a local sandbox")

        # -- shell -----------------------------------------------------------
        shell_parser = sb_subparsers.add_parser(
            "shell",
            help="Open interactive shell or run command in sandbox",
        )
        shell_parser.add_argument(
            "--local",
            action="store_true",
            help="Target a local sandbox",
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
        shell_parser.add_argument("name", help="Sandbox name")
        shell_parser.add_argument(
            "shell_command",
            nargs=argparse.REMAINDER,
            help="Command to run (optional, opens interactive shell if omitted)",
        )

        # -- exec ------------------------------------------------------------
        exec_parser = sb_subparsers.add_parser(
            "exec",
            help="Execute command in sandbox (non-interactive)",
        )
        exec_parser.add_argument(
            "--local",
            action="store_true",
            help="Target a local sandbox",
        )
        exec_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )
        exec_parser.add_argument("name", help="Sandbox name")
        exec_parser.add_argument(
            "exec_command",
            nargs=argparse.REMAINDER,
            help="Command to execute",
        )


# ---------------------------------------------------------------------------
# execute() dispatcher
# ---------------------------------------------------------------------------


def execute(args: argparse.Namespace) -> int:
    """Execute sandbox command based on subcommand."""
    cmd = getattr(args, "sandbox_command", None)

    if cmd == "launch":
        return cmd_launch(args)
    elif cmd in ("ls", "list"):
        return cmd_ls(args)
    elif cmd in ("info", "get"):
        return cmd_info(args)
    elif cmd == "suspend":
        return cmd_suspend(args)
    elif cmd == "resume":
        return cmd_resume(args)
    elif cmd == "restart":
        return cmd_restart(args)
    elif cmd == "delete":
        return cmd_delete(args)
    elif cmd == "vnc":
        return cmd_vnc(args)
    elif cmd == "shell":
        return cmd_shell(args)
    elif cmd == "exec":
        return cmd_exec(args)
    else:
        print_error("Usage: cua sandbox <command>")
        print_info("Commands: launch, ls, info, suspend, resume, restart, delete, vnc, shell, exec")
        return 1


# ---------------------------------------------------------------------------
# Command implementations
# ---------------------------------------------------------------------------


def cmd_launch(args: argparse.Namespace) -> int:
    """Launch a new sandbox."""

    async def _run() -> int:
        from cua_sandbox import Sandbox

        image = _parse_image(args.image, vm=getattr(args, "vm", False))

        memory_mb = _parse_memory(args.memory) if args.memory else None
        disk_gb = _parse_disk(args.disk) if args.disk else None

        local = getattr(args, "local", False)

        try:
            if local:
                sb = await Sandbox.create(
                    image,
                    local=True,
                    name=args.name,
                )
            else:
                create_kwargs: dict[str, Any] = {}
                if args.name:
                    create_kwargs["name"] = args.name
                if args.region:
                    create_kwargs["region"] = args.region
                if getattr(args, "cpu", None):
                    create_kwargs["cpu"] = args.cpu
                if memory_mb is not None:
                    create_kwargs["memory_mb"] = memory_mb
                if disk_gb is not None:
                    create_kwargs["disk_gb"] = disk_gb
                api_key = require_api_key()
                create_kwargs["api_key"] = api_key
                sb = await Sandbox.create(image, **create_kwargs)

            name = sb.name
            await sb.disconnect()
        except Exception as e:
            import traceback

            print_error(f"Failed to launch sandbox: {e!r}\n{traceback.format_exc()}")
            return 1

        if getattr(args, "json", False):
            print_json({"name": name, "status": "ready"})
        else:
            print_success(f"Sandbox '{name}' is ready")
        return 0

    return run_async(_run())


def cmd_ls(args: argparse.Namespace) -> int:
    """List sandboxes."""

    async def _run() -> int:
        from cua_sandbox import Sandbox

        show_all = getattr(args, "all", False)
        local = getattr(args, "local", False)
        as_json = getattr(args, "json", False)

        results: list[dict] = []

        if show_all or local:
            try:
                local_list = await Sandbox.list(local=True)
                for s in local_list:
                    results.append(
                        {
                            "name": s.name,
                            "status": s.status,
                            "source": getattr(s, "source", "local"),
                        }
                    )
            except Exception:
                pass

        if show_all or not local:
            try:
                api_key = get_api_key()
                cloud_list = await Sandbox.list(local=False, api_key=api_key)
                for s in cloud_list:
                    results.append(
                        {
                            "name": s.name,
                            "status": s.status,
                            "source": "cloud",
                        }
                    )
            except Exception:
                pass

        if as_json:
            print_json(results)
            return 0

        if not results:
            print_info("No sandboxes found.")
            return 0

        print_table(
            results,
            [("name", "NAME"), ("status", "STATUS"), ("source", "SOURCE")],
        )
        return 0

    return run_async(_run())


def cmd_info(args: argparse.Namespace) -> int:
    """Get sandbox details."""
    local = getattr(args, "local", False)

    async def _run() -> int:
        from cua_sandbox import Sandbox

        try:
            kwargs: dict[str, Any] = {}
            if not local:
                kwargs["api_key"] = require_api_key()
            info = await Sandbox.get_info(args.name, local=local, **kwargs)
        except Exception as e:
            print_error(str(e))
            return 1

        if getattr(args, "json", False):
            data = {
                "name": info.name,
                "status": info.status,
            }
            for attr in ("os_type", "host", "region", "created_at", "cpu", "memory_mb", "disk_gb"):
                val = getattr(info, attr, None)
                if val is not None:
                    data[attr] = val
            print_json(data)
            return 0

        print_info(f"Name:   {info.name}")
        print_info(f"Status: {info.status}")
        for attr, label in [
            ("os_type", "OS"),
            ("host", "Host"),
            ("region", "Region"),
            ("created_at", "Created"),
        ]:
            val = getattr(info, attr, None)
            if val:
                print_info(f"{label}: {val}")
        return 0

    return run_async(_run())


def cmd_suspend(args: argparse.Namespace) -> int:
    """Suspend a sandbox."""
    local = getattr(args, "local", False)

    async def _run() -> int:
        from cua_sandbox import Sandbox

        try:
            kwargs: dict[str, Any] = {}
            if not local:
                kwargs["api_key"] = require_api_key()
            await Sandbox.suspend(args.name, local=local, **kwargs)
        except Exception as e:
            print_error(f"Failed to suspend sandbox: {e}")
            return 1
        print_success(f"Sandbox '{args.name}' is suspending.")
        return 0

    return run_async(_run())


def cmd_resume(args: argparse.Namespace) -> int:
    """Resume a suspended sandbox."""
    local = getattr(args, "local", False)

    async def _run() -> int:
        from cua_sandbox import Sandbox

        try:
            kwargs: dict[str, Any] = {}
            if not local:
                kwargs["api_key"] = require_api_key()
            await Sandbox.resume(args.name, local=local, **kwargs)
        except Exception as e:
            print_error(f"Failed to resume sandbox: {e}")
            return 1
        print_success(f"Sandbox '{args.name}' is resuming.")
        return 0

    return run_async(_run())


def cmd_restart(args: argparse.Namespace) -> int:
    """Restart a sandbox."""
    local = getattr(args, "local", False)

    async def _run() -> int:
        from cua_sandbox import Sandbox

        try:
            kwargs: dict[str, Any] = {}
            if not local:
                kwargs["api_key"] = require_api_key()
            await Sandbox.restart(args.name, local=local, **kwargs)
        except Exception as e:
            print_error(f"Failed to restart sandbox: {e}")
            return 1
        print_success(f"Sandbox '{args.name}' is restarting.")
        return 0

    return run_async(_run())


def cmd_delete(args: argparse.Namespace) -> int:
    """Delete a sandbox."""
    local = getattr(args, "local", False)
    force = getattr(args, "force", False)

    if not force:
        import sys

        if not sys.stdin.isatty():
            force = True
        else:
            try:
                answer = input(f"Delete sandbox '{args.name}'? [y/N] ").strip().lower()
            except (EOFError, KeyboardInterrupt):
                print()
                return 1
            if answer not in ("y", "yes"):
                print_info("Aborted.")
                return 0

    async def _run() -> int:
        from cua_sandbox import Sandbox

        try:
            kwargs: dict[str, Any] = {}
            if not local:
                kwargs["api_key"] = require_api_key()
            await Sandbox.delete(args.name, local=local, **kwargs)
        except Exception as e:
            print_error(f"Failed to delete sandbox: {e}")
            return 1
        print_success(f"Sandbox '{args.name}' is being deleted.")
        return 0

    return run_async(_run())


def cmd_vnc(args: argparse.Namespace) -> int:
    """Open sandbox in browser via VNC."""
    local = getattr(args, "local", False)

    async def _run() -> int:
        from cua_sandbox import Sandbox

        try:
            kwargs: dict[str, Any] = {}
            if not local:
                kwargs["api_key"] = require_api_key()
            sb = await Sandbox.connect(args.name, local=local, **kwargs)
            vnc_url = await sb.get_display_url(share=True)
            await sb.disconnect()
        except Exception as e:
            print_error(f"Failed to get VNC URL: {e}")
            return 1

        print_info(f"Opening VNC: {vnc_url}")
        webbrowser.open(vnc_url)
        return 0

    return run_async(_run())


# ---------------------------------------------------------------------------
# PTY / WebSocket helpers (shell / exec)
# ---------------------------------------------------------------------------


def _default_shell() -> str:
    return "powershell" if sys.platform == "win32" else "bash"


async def _shell_interactive(
    name: str,
    api_url: str,
    api_key: Optional[str],
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

    headers: dict[str, str] = {
        "X-Container-Name": name,
        "Content-Type": "application/json",
    }
    ws_params: dict[str, str] = {
        "container_name": name,
    }
    if api_key:
        headers["X-API-Key"] = api_key
        ws_params["api_key"] = api_key

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
    api_key: Optional[str],
    command: str,
) -> dict:
    """Execute a command non-interactively and return result."""
    headers: dict[str, str] = {
        "X-Container-Name": name,
        "Content-Type": "application/json",
    }
    if api_key:
        headers["X-API-Key"] = api_key

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
            for line in text.splitlines():
                if line.startswith("data: "):
                    try:
                        return json.loads(line[6:])
                    except json.JSONDecodeError:
                        pass
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
    local = getattr(args, "local", False)

    async def _run() -> int:
        try:
            api_url, api_key = await _get_sandbox_api_url(args.name, local)
        except ValueError as e:
            print_error(str(e))
            return 1

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
    local = getattr(args, "local", False)

    if not command:
        print_error("No command provided")
        print_info("Usage: cua sb exec <name> <command>")
        return 1

    async def _run() -> int:
        try:
            api_url, api_key = await _get_sandbox_api_url(args.name, local)
        except ValueError as e:
            print_error(str(e))
            return 1

        result = await _exec_noninteractive(args.name, api_url, api_key, command)

        if getattr(args, "json", False):
            print_json(result)
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
