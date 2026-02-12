"""MCP server command for CUA CLI.

Provides a Model Context Protocol server that exposes CUA functionality
to AI assistants like Claude.
"""

import argparse
import json
import logging
import os
import sys
from enum import Enum
from typing import TYPE_CHECKING, Any, Optional

if TYPE_CHECKING:
    from mcp.server.fastmcp import FastMCP

# Set up logging to stderr
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    stream=sys.stderr,
)
logger = logging.getLogger("cua-mcp")


class Permission(Enum):
    """MCP permission types."""

    # Sandbox management
    SANDBOX_LIST = "sandbox:list"
    SANDBOX_CREATE = "sandbox:create"
    SANDBOX_DELETE = "sandbox:delete"
    SANDBOX_START = "sandbox:start"
    SANDBOX_STOP = "sandbox:stop"
    SANDBOX_RESTART = "sandbox:restart"
    SANDBOX_SUSPEND = "sandbox:suspend"
    SANDBOX_GET = "sandbox:get"
    SANDBOX_VNC = "sandbox:vnc"

    # Computer control
    COMPUTER_SCREENSHOT = "computer:screenshot"
    COMPUTER_CLICK = "computer:click"
    COMPUTER_TYPE = "computer:type"
    COMPUTER_KEY = "computer:key"
    COMPUTER_SCROLL = "computer:scroll"
    COMPUTER_DRAG = "computer:drag"
    COMPUTER_HOTKEY = "computer:hotkey"
    COMPUTER_CLIPBOARD = "computer:clipboard"
    COMPUTER_FILE = "computer:file"
    COMPUTER_SHELL = "computer:shell"
    COMPUTER_WINDOW = "computer:window"

    # Skills
    SKILLS_LIST = "skills:list"
    SKILLS_READ = "skills:read"
    SKILLS_RECORD = "skills:record"
    SKILLS_DELETE = "skills:delete"


# Permission groups for convenience
PERMISSION_GROUPS = {
    "sandbox:all": [
        Permission.SANDBOX_LIST,
        Permission.SANDBOX_CREATE,
        Permission.SANDBOX_DELETE,
        Permission.SANDBOX_START,
        Permission.SANDBOX_STOP,
        Permission.SANDBOX_RESTART,
        Permission.SANDBOX_SUSPEND,
        Permission.SANDBOX_GET,
        Permission.SANDBOX_VNC,
    ],
    "sandbox:readonly": [
        Permission.SANDBOX_LIST,
        Permission.SANDBOX_GET,
    ],
    "computer:all": [
        Permission.COMPUTER_SCREENSHOT,
        Permission.COMPUTER_CLICK,
        Permission.COMPUTER_TYPE,
        Permission.COMPUTER_KEY,
        Permission.COMPUTER_SCROLL,
        Permission.COMPUTER_DRAG,
        Permission.COMPUTER_HOTKEY,
        Permission.COMPUTER_CLIPBOARD,
        Permission.COMPUTER_FILE,
        Permission.COMPUTER_SHELL,
        Permission.COMPUTER_WINDOW,
    ],
    "computer:readonly": [
        Permission.COMPUTER_SCREENSHOT,
    ],
    "skills:all": [
        Permission.SKILLS_LIST,
        Permission.SKILLS_READ,
        Permission.SKILLS_RECORD,
        Permission.SKILLS_DELETE,
    ],
    "skills:readonly": [
        Permission.SKILLS_LIST,
        Permission.SKILLS_READ,
    ],
    "all": list(Permission),
}


def parse_permissions(permissions_str: str) -> set[Permission]:
    """Parse permissions from a comma-separated string."""
    if not permissions_str:
        return set()

    permissions = set()
    for perm in permissions_str.split(","):
        perm = perm.strip()
        if not perm:
            continue

        # Check if it's a group
        if perm in PERMISSION_GROUPS:
            permissions.update(PERMISSION_GROUPS[perm])
        else:
            # Try to match individual permission
            try:
                permissions.add(Permission(perm))
            except ValueError:
                logger.warning(f"Unknown permission: {perm}")

    return permissions


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the serve-mcp command."""
    mcp_parser = subparsers.add_parser(
        "serve-mcp",
        help="Start MCP server for AI assistants",
        description="Start a Model Context Protocol server that exposes CUA functionality",
    )

    mcp_parser.add_argument(
        "--permissions",
        type=str,
        default="",
        help="Comma-separated list of permissions (default: from CUA_MCP_PERMISSIONS env var)",
    )

    mcp_parser.add_argument(
        "--sandbox",
        type=str,
        default="",
        help="Default sandbox name for computer commands (default: from CUA_SANDBOX env var)",
    )


def execute(args: argparse.Namespace) -> int:
    """Execute the serve-mcp command."""
    try:
        from mcp.server.fastmcp import FastMCP  # noqa: F401
    except ImportError:
        print("MCP support not installed. Run: pip install cua-cli[mcp]", file=sys.stderr)
        return 1

    # Parse permissions from args or env var
    permissions_str = args.permissions or os.environ.get("CUA_MCP_PERMISSIONS", "")
    permissions = parse_permissions(permissions_str)

    if not permissions:
        # Default to all permissions if none specified
        logger.info("No permissions specified, granting all permissions")
        permissions = set(Permission)
    else:
        logger.info(f"Enabled permissions: {[p.value for p in permissions]}")

    # Get default sandbox
    default_sandbox = args.sandbox or os.environ.get("CUA_SANDBOX", "")

    # Create and run the MCP server
    import anyio

    anyio.run(lambda: _run_mcp_server(permissions, default_sandbox))
    return 0


async def _run_mcp_server(permissions: set[Permission], default_sandbox: str) -> None:
    """Create and run the MCP server."""
    from mcp.server.fastmcp import FastMCP

    server = FastMCP(name="cua")

    # Register tools based on permissions
    await _register_sandbox_tools(server, permissions)
    await _register_computer_tools(server, permissions, default_sandbox)
    await _register_skills_tools(server, permissions)

    logger.info("Starting CUA MCP server...")
    await server.run_stdio_async()


async def _register_sandbox_tools(server: "FastMCP", permissions: set[Permission]) -> None:
    """Register sandbox management tools."""
    from mcp.server.fastmcp import Context

    if Permission.SANDBOX_LIST in permissions:

        @server.tool()
        async def sandbox_list(ctx: Context) -> str:
            """List all cloud sandboxes."""
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                vms = await provider.list_vms()
                return json.dumps(
                    [
                        {
                            "name": vm.name,
                            "status": vm.status,
                            "os_type": vm.os_type,
                            "created_at": vm.created_at,
                        }
                        for vm in vms
                    ],
                    indent=2,
                )

    if Permission.SANDBOX_CREATE in permissions:

        @server.tool()
        async def sandbox_create(
            ctx: Context,
            os_type: str = "linux",
            size: str = "medium",
            region: str = "north-america",
        ) -> str:
            """Create a new cloud sandbox.

            Args:
                os_type: Operating system (linux, macos, windows)
                size: VM size (small, medium, large, xlarge)
                region: Region (north-america, europe, asia)
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                vm = await provider.create_vm(os_type=os_type, size=size, region=region)
                return json.dumps(
                    {
                        "name": vm.name,
                        "status": vm.status,
                        "os_type": vm.os_type,
                        "message": f"Created sandbox: {vm.name}",
                    },
                    indent=2,
                )

    if Permission.SANDBOX_GET in permissions:

        @server.tool()
        async def sandbox_get(ctx: Context, name: str) -> str:
            """Get details for a specific sandbox.

            Args:
                name: Sandbox name
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                vm = await provider.get_vm(name)
                if not vm:
                    return json.dumps({"error": f"Sandbox not found: {name}"})
                return json.dumps(
                    {
                        "name": vm.name,
                        "status": vm.status,
                        "os_type": vm.os_type,
                        "size": getattr(vm, "size", None),
                        "region": getattr(vm, "region", None),
                        "created_at": vm.created_at,
                        "vnc_url": getattr(vm, "vnc_url", None),
                        "server_url": getattr(vm, "server_url", None),
                    },
                    indent=2,
                )

    if Permission.SANDBOX_START in permissions:

        @server.tool()
        async def sandbox_start(ctx: Context, name: str) -> str:
            """Start a stopped sandbox.

            Args:
                name: Sandbox name
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                await provider.run_vm(name)
                return json.dumps({"success": True, "message": f"Started sandbox: {name}"})

    if Permission.SANDBOX_STOP in permissions:

        @server.tool()
        async def sandbox_stop(ctx: Context, name: str) -> str:
            """Stop a running sandbox.

            Args:
                name: Sandbox name
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                await provider.stop_vm(name)
                return json.dumps({"success": True, "message": f"Stopped sandbox: {name}"})

    if Permission.SANDBOX_RESTART in permissions:

        @server.tool()
        async def sandbox_restart(ctx: Context, name: str) -> str:
            """Restart a sandbox.

            Args:
                name: Sandbox name
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                await provider.restart_vm(name)
                return json.dumps({"success": True, "message": f"Restarted sandbox: {name}"})

    if Permission.SANDBOX_SUSPEND in permissions:

        @server.tool()
        async def sandbox_suspend(ctx: Context, name: str) -> str:
            """Suspend a running sandbox.

            Args:
                name: Sandbox name
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                await provider.suspend_vm(name)
                return json.dumps({"success": True, "message": f"Suspended sandbox: {name}"})

    if Permission.SANDBOX_DELETE in permissions:

        @server.tool()
        async def sandbox_delete(ctx: Context, name: str) -> str:
            """Delete a sandbox.

            Args:
                name: Sandbox name
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                await provider.delete_vm(name)
                return json.dumps({"success": True, "message": f"Deleted sandbox: {name}"})

    if Permission.SANDBOX_VNC in permissions:

        @server.tool()
        async def sandbox_vnc(ctx: Context, name: str) -> str:
            """Get VNC URL for a sandbox.

            Args:
                name: Sandbox name
            """
            from computer.providers import VMProviderFactory, VMProviderType
            from cua_cli.auth.store import require_api_key

            api_key = require_api_key()
            provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
            async with provider:
                vm = await provider.get_vm(name)
                if not vm:
                    return json.dumps({"error": f"Sandbox not found: {name}"})
                vnc_url = getattr(vm, "vnc_url", None)
                if not vnc_url:
                    return json.dumps({"error": "VNC URL not available"})
                return json.dumps({"vnc_url": vnc_url})


async def _register_computer_tools(
    server: "FastMCP",
    permissions: set[Permission],
    default_sandbox: str,
) -> None:
    """Register computer control tools that proxy to computer-server."""
    import aiohttp
    from cua_cli.auth.store import get_api_key
    from mcp.server.fastmcp import Context
    from mcp.server.fastmcp.utilities.types import Image

    async def _get_server_url(sandbox_name: str) -> Optional[str]:
        """Get the computer-server URL for a sandbox."""
        from computer.providers import VMProviderFactory, VMProviderType

        name = sandbox_name or default_sandbox
        if not name:
            raise ValueError("No sandbox specified. Use --sandbox or set CUA_SANDBOX env var")

        api_key = get_api_key()
        if not api_key:
            raise ValueError("Not authenticated. Run 'cua auth login' first")

        provider = VMProviderFactory.create_provider(VMProviderType.CLOUD, api_key=api_key)
        async with provider:
            vm = await provider.get_vm(name)
            if not vm:
                raise ValueError(f"Sandbox not found: {name}")
            server_url = getattr(vm, "server_url", None)
            if not server_url:
                raise ValueError(f"Sandbox {name} is not running or has no server URL")
            return server_url

    async def _send_command(sandbox_name: str, command: str, params: dict) -> dict:
        """Send a command to the computer-server."""
        server_url = await _get_server_url(sandbox_name)
        api_key = get_api_key()

        headers = {
            "Content-Type": "application/json",
            "X-API-Key": api_key,
            "X-Container-Name": sandbox_name or default_sandbox,
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(
                f"{server_url}/cmd",
                json={"command": command, "params": params},
                headers=headers,
            ) as resp:
                # Read SSE response
                text = await resp.text()
                # Parse SSE data lines
                for line in text.split("\n"):
                    if line.startswith("data: "):
                        return json.loads(line[6:])
                return {"success": False, "error": "No response from server"}

    if Permission.COMPUTER_SCREENSHOT in permissions:

        @server.tool()
        async def computer_screenshot(ctx: Context, sandbox: str = "") -> Any:
            """Take a screenshot of the sandbox screen.

            Args:
                sandbox: Sandbox name (optional, uses default if not specified)
            """
            result = await _send_command(sandbox, "screenshot", {})
            if result.get("success") and result.get("image_data"):
                import base64

                return Image(format="png", data=base64.b64decode(result["image_data"]))
            return json.dumps(result)

    if Permission.COMPUTER_CLICK in permissions:

        @server.tool()
        async def computer_click(
            ctx: Context,
            x: int,
            y: int,
            button: str = "left",
            sandbox: str = "",
        ) -> str:
            """Click at coordinates on the screen.

            Args:
                x: X coordinate
                y: Y coordinate
                button: Mouse button (left, right, middle)
                sandbox: Sandbox name (optional)
            """
            if button == "left":
                result = await _send_command(sandbox, "left_click", {"x": x, "y": y})
            elif button == "right":
                result = await _send_command(sandbox, "right_click", {"x": x, "y": y})
            else:
                result = await _send_command(sandbox, "left_click", {"x": x, "y": y})
            return json.dumps(result)

        @server.tool()
        async def computer_double_click(
            ctx: Context,
            x: int,
            y: int,
            sandbox: str = "",
        ) -> str:
            """Double-click at coordinates on the screen.

            Args:
                x: X coordinate
                y: Y coordinate
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "double_click", {"x": x, "y": y})
            return json.dumps(result)

    if Permission.COMPUTER_TYPE in permissions:

        @server.tool()
        async def computer_type(ctx: Context, text: str, sandbox: str = "") -> str:
            """Type text on the keyboard.

            Args:
                text: Text to type
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "type_text", {"text": text})
            return json.dumps(result)

    if Permission.COMPUTER_KEY in permissions:

        @server.tool()
        async def computer_key(ctx: Context, key: str, sandbox: str = "") -> str:
            """Press a key on the keyboard.

            Args:
                key: Key to press (e.g., "enter", "tab", "escape")
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "press_key", {"key": key})
            return json.dumps(result)

    if Permission.COMPUTER_HOTKEY in permissions:

        @server.tool()
        async def computer_hotkey(ctx: Context, keys: str, sandbox: str = "") -> str:
            """Press a keyboard shortcut.

            Args:
                keys: Keys to press (e.g., "cmd+c", "ctrl+shift+s")
                sandbox: Sandbox name (optional)
            """
            key_list = keys.replace("-", "+").split("+")
            result = await _send_command(sandbox, "hotkey", {"keys": key_list})
            return json.dumps(result)

    if Permission.COMPUTER_SCROLL in permissions:

        @server.tool()
        async def computer_scroll(
            ctx: Context,
            direction: str = "down",
            amount: int = 3,
            sandbox: str = "",
        ) -> str:
            """Scroll the screen.

            Args:
                direction: Scroll direction (up, down, left, right)
                amount: Number of scroll clicks
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(
                sandbox, "scroll_direction", {"direction": direction, "clicks": amount}
            )
            return json.dumps(result)

    if Permission.COMPUTER_DRAG in permissions:

        @server.tool()
        async def computer_drag(
            ctx: Context,
            start_x: int,
            start_y: int,
            end_x: int,
            end_y: int,
            sandbox: str = "",
        ) -> str:
            """Drag from one point to another.

            Args:
                start_x: Starting X coordinate
                start_y: Starting Y coordinate
                end_x: Ending X coordinate
                end_y: Ending Y coordinate
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(
                sandbox,
                "drag_to",
                {"start_x": start_x, "start_y": start_y, "end_x": end_x, "end_y": end_y},
            )
            return json.dumps(result)

    if Permission.COMPUTER_CLIPBOARD in permissions:

        @server.tool()
        async def computer_clipboard_get(ctx: Context, sandbox: str = "") -> str:
            """Get clipboard contents.

            Args:
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "copy_to_clipboard", {})
            return json.dumps(result)

        @server.tool()
        async def computer_clipboard_set(ctx: Context, text: str, sandbox: str = "") -> str:
            """Set clipboard contents.

            Args:
                text: Text to copy to clipboard
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "set_clipboard", {"text": text})
            return json.dumps(result)

    if Permission.COMPUTER_FILE in permissions:

        @server.tool()
        async def computer_file_read(ctx: Context, path: str, sandbox: str = "") -> str:
            """Read a file from the sandbox.

            Args:
                path: Path to the file
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "read_text", {"path": path})
            return json.dumps(result)

        @server.tool()
        async def computer_file_write(
            ctx: Context, path: str, content: str, sandbox: str = ""
        ) -> str:
            """Write a file to the sandbox.

            Args:
                path: Path to the file
                content: File content
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "write_text", {"path": path, "content": content})
            return json.dumps(result)

        @server.tool()
        async def computer_file_list(ctx: Context, path: str = ".", sandbox: str = "") -> str:
            """List files in a directory.

            Args:
                path: Directory path
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "list_dir", {"path": path})
            return json.dumps(result)

    if Permission.COMPUTER_SHELL in permissions:

        @server.tool()
        async def computer_shell(ctx: Context, command: str, sandbox: str = "") -> str:
            """Run a shell command in the sandbox.

            Args:
                command: Shell command to run
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "run_command", {"command": command})
            return json.dumps(result)

    if Permission.COMPUTER_WINDOW in permissions:

        @server.tool()
        async def computer_window_list(ctx: Context, sandbox: str = "") -> str:
            """List open windows.

            Args:
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "get_application_windows", {})
            return json.dumps(result)

        @server.tool()
        async def computer_window_open(ctx: Context, path: str, sandbox: str = "") -> str:
            """Open a file or URL.

            Args:
                path: Path to file or URL to open
                sandbox: Sandbox name (optional)
            """
            result = await _send_command(sandbox, "open", {"path": path})
            return json.dumps(result)


async def _register_skills_tools(server: "FastMCP", permissions: set[Permission]) -> None:
    """Register skills management tools."""
    from pathlib import Path

    from mcp.server.fastmcp import Context

    SKILLS_DIR = Path.home() / ".cua" / "skills"

    if Permission.SKILLS_LIST in permissions:

        @server.tool()
        async def skills_list(ctx: Context) -> str:
            """List all recorded skills."""
            if not SKILLS_DIR.exists():
                return json.dumps([])

            skills = []
            for skill_dir in SKILLS_DIR.iterdir():
                if skill_dir.is_dir() and (skill_dir / "SKILL.md").exists():
                    skill_file = skill_dir / "SKILL.md"
                    content = skill_file.read_text()

                    # Extract title from markdown
                    title = skill_dir.name
                    for line in content.split("\n"):
                        if line.startswith("# "):
                            title = line[2:].strip()
                            break

                    # Count trajectory steps
                    trajectory_dir = skill_dir / "trajectory"
                    step_count = (
                        len(list(trajectory_dir.glob("step_*.md")))
                        if trajectory_dir.exists()
                        else 0
                    )

                    skills.append(
                        {
                            "name": skill_dir.name,
                            "title": title,
                            "steps": step_count,
                        }
                    )

            return json.dumps(skills, indent=2)

    if Permission.SKILLS_READ in permissions:

        @server.tool()
        async def skills_read(ctx: Context, name: str) -> str:
            """Read a skill's content.

            Args:
                name: Skill name
            """
            skill_dir = SKILLS_DIR / name
            skill_file = skill_dir / "SKILL.md"

            if not skill_file.exists():
                return json.dumps({"error": f"Skill not found: {name}"})

            content = skill_file.read_text()

            # Also include trajectory steps
            trajectory_dir = skill_dir / "trajectory"
            steps = []
            if trajectory_dir.exists():
                for step_file in sorted(trajectory_dir.glob("step_*.md")):
                    steps.append(
                        {
                            "file": step_file.name,
                            "content": step_file.read_text(),
                        }
                    )

            return json.dumps(
                {
                    "name": name,
                    "content": content,
                    "steps": steps,
                },
                indent=2,
            )

    if Permission.SKILLS_DELETE in permissions:

        @server.tool()
        async def skills_delete(ctx: Context, name: str) -> str:
            """Delete a skill.

            Args:
                name: Skill name
            """
            import shutil

            skill_dir = SKILLS_DIR / name

            if not skill_dir.exists():
                return json.dumps({"error": f"Skill not found: {name}"})

            shutil.rmtree(skill_dir)
            return json.dumps({"success": True, "message": f"Deleted skill: {name}"})

    if Permission.SKILLS_RECORD in permissions:

        @server.tool()
        async def skills_record(
            ctx: Context,
            name: str,
            sandbox: str = "",
            port: int = 8765,
        ) -> str:
            """Start recording a skill.

            This starts a WebSocket server that receives screen recordings.
            Use the CUA browser extension or screen recorder to send frames.

            Args:
                name: Name for the skill
                sandbox: Sandbox name (optional)
                port: WebSocket port (default: 8765)
            """
            return json.dumps(
                {
                    "message": f"To record skill '{name}', use 'cua skills record {name}' from the terminal",
                    "instructions": [
                        f"1. Run: cua skills record {name}",
                        "2. Use the CUA browser extension or screen recorder",
                        "3. Perform the actions you want to record",
                        "4. Stop the recording when done",
                    ],
                },
                indent=2,
            )
