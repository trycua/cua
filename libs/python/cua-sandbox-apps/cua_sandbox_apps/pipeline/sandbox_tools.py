"""MCP tools for sandboxed app installation and testing.

Creator agents get these tools instead of Bash. All execution happens
inside ephemeral cua_sandbox VMs — nothing runs on the host.
"""

from __future__ import annotations

import asyncio
import logging
import uuid
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

# Global registry for cleanup_all_sandboxes() only
_all_sandboxes: dict[str, Any] = {}


def make_sandbox_tools(
    evidence_dir: Path | None = None, sandbox_registry: dict[str, Any] | None = None
):
    """Create MCP tools for sandbox lifecycle + file/exec operations.

    Args:
        evidence_dir: Where to save screenshots/icons.
        sandbox_registry: Per-agent dict to track this agent's sandboxes.
            If None, creates a new isolated dict. Never shared between agents.

    Returns list of tools for use with create_sdk_mcp_server().
    """
    from claude_agent_sdk import tool

    # Each agent gets its own sandbox registry — no cross-agent interference
    sandboxes = sandbox_registry if sandbox_registry is not None else {}

    @tool(
        "create_sandbox",
        "Create a new sandbox VM for the given OS. Returns a sandbox name to use with other sandbox tools.",
        {
            "type": "object",
            "properties": {
                "os": {
                    "type": "string",
                    "enum": ["linux", "windows", "macos", "android"],
                    "description": "Target operating system",
                },
            },
            "required": ["os"],
        },
    )
    async def create_sandbox(args: dict[str, Any]) -> dict[str, Any]:
        os_type = args["os"]
        name = f"{os_type}-{uuid.uuid4().hex[:8]}"

        try:
            from cua_sandbox import Image, Sandbox

            image_map = {
                "linux": Image.linux,
                "windows": Image.windows,
                "macos": Image.macos,
                "android": Image.android,
            }

            if os_type not in image_map:
                return {"content": [{"type": "text", "text": f"ERROR: unsupported OS '{os_type}'"}]}

            img = image_map[os_type]()
            sb = await Sandbox.create(img, local=True, name=name)
            sandboxes[name] = sb
            _all_sandboxes[name] = sb

            logger.info("Created sandbox %s (os=%s)", name, os_type)
            return {
                "content": [
                    {
                        "type": "text",
                        "text": f"OK: sandbox '{name}' created (os={os_type}). Use this name with other sandbox tools.",
                    }
                ]
            }

        except Exception as e:
            logger.error("Failed to create sandbox %s: %s", name, e)
            return {"content": [{"type": "text", "text": f"ERROR: {e}"}]}

    @tool(
        "sandbox_run",
        "Run a shell command inside a sandbox. Returns stdout, stderr, and exit code.",
        {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Sandbox name from create_sandbox"},
                "command": {"type": "string", "description": "Shell command to run"},
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 120)",
                    "default": 120,
                },
            },
            "required": ["name", "command"],
        },
    )
    async def sandbox_run(args: dict[str, Any]) -> dict[str, Any]:
        name = args["name"]
        command = args["command"]
        timeout = args.get("timeout", 120)

        sb = sandboxes.get(name)
        if not sb:
            return {
                "content": [
                    {
                        "type": "text",
                        "text": f"ERROR: sandbox '{name}' not found. Create one first.",
                    }
                ]
            }

        logger.info("[%s] $ %s", name, command[:120])
        try:
            result = await asyncio.wait_for(
                sb.shell.run(command),
                timeout=timeout,
            )
            status = "OK" if result.returncode == 0 else f"FAIL(exit={result.returncode})"
            logger.info("[%s] %s | stdout=%d bytes", name, status, len(result.stdout))
            output = f"EXIT CODE: {result.returncode}\n\nSTDOUT:\n{result.stdout[:10000]}\n\nSTDERR:\n{result.stderr[:5000]}"
            return {"content": [{"type": "text", "text": output}]}
        except asyncio.TimeoutError:
            return {
                "content": [{"type": "text", "text": f"ERROR: command timed out after {timeout}s"}]
            }
        except Exception as e:
            return {"content": [{"type": "text", "text": f"ERROR: {e}"}]}

    @tool(
        "sandbox_write",
        "Write a file inside a sandbox.",
        {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Sandbox name"},
                "path": {
                    "type": "string",
                    "description": "Absolute path inside the sandbox (e.g. /tmp/install.sh)",
                },
                "content": {"type": "string", "description": "File content to write"},
                "executable": {
                    "type": "boolean",
                    "description": "Make the file executable (default false)",
                    "default": False,
                },
            },
            "required": ["name", "path", "content"],
        },
    )
    async def sandbox_write(args: dict[str, Any]) -> dict[str, Any]:
        name = args["name"]
        path = args["path"]
        content = args["content"]
        executable = args.get("executable", False)

        sb = sandboxes.get(name)
        if not sb:
            return {"content": [{"type": "text", "text": f"ERROR: sandbox '{name}' not found."}]}

        logger.info("[%s] write %s (%d bytes, exec=%s)", name, path, len(content), executable)
        try:
            # Write via shell — works on all OS types
            escaped = content.replace("\\", "\\\\").replace("'", "'\\''")
            await sb.shell.run(f"cat > {path} << 'SANDBOX_EOF'\n{content}\nSANDBOX_EOF")
            if executable:
                await sb.shell.run(f"chmod +x {path}")
            return {
                "content": [{"type": "text", "text": f"OK: wrote {len(content)} bytes to {path}"}]
            }
        except Exception as e:
            return {"content": [{"type": "text", "text": f"ERROR: {e}"}]}

    @tool(
        "sandbox_read",
        "Read a file from inside a sandbox.",
        {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Sandbox name"},
                "path": {"type": "string", "description": "Absolute path inside the sandbox"},
                "max_lines": {
                    "type": "integer",
                    "description": "Max lines to return (default 200)",
                    "default": 200,
                },
            },
            "required": ["name", "path"],
        },
    )
    async def sandbox_read(args: dict[str, Any]) -> dict[str, Any]:
        name = args["name"]
        path = args["path"]
        max_lines = args.get("max_lines", 200)

        sb = sandboxes.get(name)
        if not sb:
            return {"content": [{"type": "text", "text": f"ERROR: sandbox '{name}' not found."}]}

        try:
            result = await sb.shell.run(f"head -n {max_lines} {path}")
            if result.returncode != 0:
                return {"content": [{"type": "text", "text": f"ERROR: {result.stderr[:2000]}"}]}
            return {"content": [{"type": "text", "text": result.stdout[:20000]}]}
        except Exception as e:
            return {"content": [{"type": "text", "text": f"ERROR: {e}"}]}

    @tool(
        "sandbox_screenshot",
        "Take a screenshot of the sandbox display. Returns the screenshot file path.",
        {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Sandbox name"},
            },
            "required": ["name"],
        },
    )
    async def sandbox_screenshot(args: dict[str, Any]) -> dict[str, Any]:
        name = args["name"]

        sb = sandboxes.get(name)
        if not sb:
            return {"content": [{"type": "text", "text": f"ERROR: sandbox '{name}' not found."}]}

        try:
            screenshot = await sb.screenshot()
            if evidence_dir:
                evidence_dir.mkdir(parents=True, exist_ok=True)
                out_path = evidence_dir / f"{name}.jpg"
                if hasattr(screenshot, "save"):
                    # PIL Image — save as JPEG
                    screenshot = screenshot.convert("RGB")
                    screenshot.save(str(out_path), "JPEG", quality=85)
                elif isinstance(screenshot, bytes):
                    # Raw PNG bytes — convert to JPEG
                    try:
                        import io

                        from PIL import Image as PILImage

                        img = PILImage.open(io.BytesIO(screenshot)).convert("RGB")
                        img.save(str(out_path), "JPEG", quality=85)
                    except ImportError:
                        # No PIL — save as PNG with .jpg extension (fallback)
                        out_path = evidence_dir / f"{name}.png"
                        out_path.write_bytes(screenshot)
                return {
                    "content": [{"type": "text", "text": f"OK: screenshot saved to {out_path}"}]
                }
            return {
                "content": [
                    {"type": "text", "text": "OK: screenshot taken (no evidence_dir configured)"}
                ]
            }
        except Exception as e:
            return {"content": [{"type": "text", "text": f"ERROR: {e}"}]}

    @tool(
        "delete_sandbox",
        "Delete a sandbox and free its resources. Always call this when done.",
        {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Sandbox name to delete"},
            },
            "required": ["name"],
        },
    )
    async def delete_sandbox(args: dict[str, Any]) -> dict[str, Any]:
        name = args["name"]

        sb = sandboxes.pop(name, None)
        _all_sandboxes.pop(name, None)
        if not sb:
            return {
                "content": [
                    {"type": "text", "text": f"OK: sandbox '{name}' already deleted or not found."}
                ]
            }

        try:
            await sb.destroy()
            logger.info("Deleted sandbox %s", name)
            return {"content": [{"type": "text", "text": f"OK: sandbox '{name}' deleted."}]}
        except Exception as e:
            logger.warning("Error deleting sandbox %s: %s", name, e)
            return {"content": [{"type": "text", "text": f"WARN: sandbox deleted with error: {e}"}]}

    return [
        create_sandbox,
        sandbox_run,
        sandbox_write,
        sandbox_read,
        sandbox_screenshot,
        delete_sandbox,
    ]


async def cleanup_all_sandboxes() -> None:
    """Force-delete all live sandboxes. Called by orchestrator on shutdown."""
    for name in list(_all_sandboxes):
        sb = _all_sandboxes.pop(name, None)
        if sb:
            try:
                await sb.destroy()
                logger.info("Cleanup: deleted sandbox %s", name)
            except Exception as e:
                logger.warning("Cleanup failed for %s: %s", name, e)
