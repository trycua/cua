# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cua-sandbox", "httpx>=0.27,<1"]
# ///
"""
Demo 3: OSWorld desktop on Fleet (or local QEMU) with the Cua Driver MCP.
Claims an OSWorld disk, screenshots it through the OSWorld control server, then
drives it over the cua-driver MCP endpoint baked into the image.

Usage:
    export OSWORLD_IMAGE="<registry>/<repo>@sha256:<digest>"   # containerDisk you published
    uv run samples/python/3_osworld_fleet.py                     # Fleet
    OSWORLD_LOCAL=1 OSWORLD_QCOW2=/path/to/overlay.qcow2 uv run samples/python/3_osworld_fleet.py  # local

See docs/content/docs/how-to-guides/sandbox/run-osworld-on-cloud-fleet.mdx.
"""

import asyncio
import json
import os
import re
import sys
import time
from pathlib import Path

import httpx
from cua_sandbox import Image, Sandbox

sys.stdout.reconfigure(encoding="utf-8")  # tool results contain emoji; keep Windows consoles happy

LOCAL = os.environ.get("OSWORLD_LOCAL") == "1"
IMAGE_REF = os.environ["OSWORLD_IMAGE"] if not os.environ.get("OSWORLD_LOCAL") else ""
QCOW2 = os.environ.get("OSWORLD_QCOW2", str(Path.home() / "osworld-cua" / "osworld-cua.qcow2"))
MCP_PORT = 3000

# The only line that differs between Fleet and local: where the disk comes from.
image = (
    Image.from_file(QCOW2, os_type="linux", agent_type="osworld")
    if LOCAL
    else Image.from_registry(IMAGE_REF, os_type="linux", kind="vm", agent_type="osworld")
).expose(MCP_PORT)


class Mcp:
    """Minimal MCP Streamable-HTTP client over the sandbox's port-3000 service."""

    def __init__(self, sb: Sandbox):
        self.sb, self.session, self.next_id = sb, None, 1

    async def _post(self, payload: dict) -> httpx.Response:
        headers = {
            "content-type": "application/json",
            "accept": "application/json, text/event-stream",
        }
        if self.session:
            headers["mcp-session-id"] = self.session
        if LOCAL:  # bare-metal QEMU forwards exposed guest ports to localhost
            host_port = self.sb.exposed_ports[MCP_PORT]
            async with httpx.AsyncClient(timeout=120) as c:
                return await c.post(
                    f"http://127.0.0.1:{host_port}/mcp", headers=headers, json=payload
                )
        return await self.sb.services.request(  # Fleet: authenticated service proxy
            f"port-{MCP_PORT}", method="POST", path="/mcp", headers=headers, json=payload
        )

    async def call(self, method: str, params: dict | None = None) -> dict:
        payload = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params or {}}
        self.next_id += 1
        r = await self._post(payload)
        r.raise_for_status()
        self.session = r.headers.get("mcp-session-id", self.session)
        body = r.text
        if r.headers.get("content-type", "").startswith("text/event-stream"):
            body = [ln[5:] for ln in body.splitlines() if ln.startswith("data:")][-1]
        msg = json.loads(body)
        if "error" in msg:
            raise RuntimeError(msg["error"])
        return msg["result"]

    async def notify(self, method: str) -> None:
        await self._post({"jsonrpc": "2.0", "method": method})

    async def tool(self, tool_name: str, **arguments) -> str:
        result = await self.call("tools/call", {"name": tool_name, "arguments": arguments})
        return "\n".join(c.get("text", "") for c in result.get("content", []))


async def main() -> None:
    t0 = time.time()
    async with Sandbox.ephemeral(
        image, local=LOCAL, cpu=4, memory_mb=8192, time_to_start=1500
    ) as sb:
        print(
            f"[{time.time() - t0:.0f}s] sandbox {sb.name} ready ({'local' if LOCAL else 'fleet'})"
        )

        # 1. OSWorld control server (:5000) through the normal Sandbox surface.
        print("screen:", await sb.get_dimensions())
        r = await sb.shell.run("whoami; hostname; cua-driver --version", timeout=30)
        print("shell:", r.stdout.strip().replace("\n", " | "), "rc", r.returncode)
        Path("osworld.png").write_bytes(await sb.screenshot())
        print("saved osworld.png")

        # 2. cua-driver MCP (:3000/mcp) baked into the image.
        mcp = Mcp(sb)
        info = await mcp.call(
            "initialize",
            {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "osworld-tutorial", "version": "0.1"},
            },
        )
        await mcp.notify("notifications/initialized")
        print("mcp server:", info["serverInfo"])
        tools = [t["name"] for t in (await mcp.call("tools/list"))["tools"]]
        print(f"mcp tools: {len(tools)} e.g. {tools[:8]}")
        await mcp.tool("start_session", session="osworld-tutorial")
        print("screen size via MCP:", await mcp.tool("get_screen_size"))
        print("apps via MCP:", (await mcp.tool("list_apps"))[:300])
        launched = await mcp.tool("launch_app", name="gnome-calculator", session="osworld-tutorial")
        print("launch_app:", launched[:300])
        await asyncio.sleep(3)
        if pid := re.search(r"pid (\d+)", launched):
            print(
                "bring_to_front:",
                await mcp.tool("bring_to_front", pid=int(pid.group(1)), session="osworld-tutorial"),
            )
            await asyncio.sleep(1)
        print("windows via MCP:", (await mcp.tool("list_windows"))[:400])
        await mcp.tool("end_session", session="osworld-tutorial")
        Path("osworld-after.png").write_bytes(await sb.screenshot())
        print("saved osworld-after.png")
    print(
        f"[{time.time() - t0:.0f}s] released; pool deleted"
        if not LOCAL
        else f"[{time.time() - t0:.0f}s] VM stopped"
    )


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
