"""Browse the web from an agent with Cua: a fresh-agent session, scripted.

Drives ONLY the host `cua mcp` server, as any MCP client would (the official
MCP Python SDK over stdio), following the cua-sandboxes skill's "Browse the
web" recipe:

  images_list -> sandbox_create {browser: true} -> call_tool (the in-sandbox
  cua-driver browser tools) -> read / fill / submit / screenshot ->
  sandbox_delete

Deterministic and LLM-free. It starts a sandbox, so it is opt-in:

  CUA_E2E_BROWSE=1 python tests/e2e/browse/e2e_browse.py [--cloud]

Environment:
  CUA_BIN            the cua binary (default: `cua` on PATH)
  CUA_IMAGE_LINUX    image override for the `linux` alias (a locally built
                     linux, for example)
  CUA_E2E_EXTERNAL   0 skips the public page (https://example.com)
  CUA_E2E_OUT        directory for transcript.json and the screenshot

Exit code 0 means every step passed. The sandbox is always deleted.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import json
import os
import pathlib
import sys
import time
import uuid

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

HERE = pathlib.Path(__file__).resolve().parent
FORM_SERVER = HERE / "form_server.py"
FORM_PORT = 18088
MAX_READS = 20  # bounded polling everywhere


class Step(Exception):
    pass


class Agent:
    """A minimal MCP client that records every tool call it makes."""

    def __init__(self, session: ClientSession):
        self.s = session
        self.transcript: list[dict] = []

    async def call(self, tool: str, args: dict) -> tuple[dict | None, list, bool]:
        t0 = time.monotonic()
        r = await self.s.call_tool(tool, args)
        dt = time.monotonic() - t0
        is_error = bool(getattr(r, "is_error", getattr(r, "isError", False)))
        structured = getattr(r, "structured_content", None) or getattr(r, "structuredContent", None)
        texts = [c.text for c in r.content if c.type == "text"]
        images = [c for c in r.content if c.type == "image"]
        if structured is None and texts:
            try:
                structured = json.loads(texts[0])
            except ValueError:
                structured = None
        self.transcript.append(
            {
                "tool": tool,
                "inner_tool": args.get("tool"),
                "seconds": round(dt, 3),
                "is_error": is_error,
                "summary": (texts[0][:160] if texts else f"{len(images)} image(s)"),
            }
        )
        return structured, r.content, is_error

    async def ok(self, tool: str, args: dict) -> dict:
        structured, content, is_error = await self.call(tool, args)
        if is_error:
            text = " ".join(c.text for c in content if c.type == "text")
            raise Step(f"{tool} {args.get('tool', '')} failed: {text[:600]}")
        return structured or {}


def refs(snapshot: dict) -> list[dict]:
    return list(snapshot.get("refs", [])) + list(snapshot.get("content_refs", []))


def find_ref(snapshot: dict, role: str, name: str) -> str:
    for r in refs(snapshot):
        if r.get("role") == role and (r.get("name") or "") == name:
            return r["ref"]
    raise Step(f"no {role} {name!r} in snapshot: {snapshot.get('outline')}")


async def run(cloud: bool, out: pathlib.Path, result: dict) -> dict:
    cua = os.environ.get("CUA_BIN", "cua")
    params = StdioServerParameters(command=cua, args=["mcp"], env=dict(os.environ))
    name = f"cua-e2e-browse-{uuid.uuid4().hex[:6]}"
    started = time.monotonic()
    result.update({"sandbox": name, "location": "cloud" if cloud else "local"})
    async with stdio_client(params) as (r, w):
        async with ClientSession(r, w) as s:
            await s.initialize()
            agent = Agent(s)
            tools = {t.name for t in (await s.list_tools()).tools}
            for needed in ("images_list", "sandbox_create", "call_tool", "sandbox_delete"):
                if needed not in tools:
                    raise Step(f"cua mcp does not serve {needed}")
            space = None
            try:
                # 1. Discover an image that ships a browser the tools drive.
                # region docs:discover
                cat = await agent.ok("images_list", {"browser": True})
                images = [i for i in cat["images"] if i["browser_tools"]]
                # endregion docs:discover
                if not any(i["ref"] == "ghcr.io/trycua/linux:24.04" for i in images):
                    raise Step(f"the canonical Linux image is not a browser image: {images}")

                # 2. One step: sandbox + browser, optionally opening a page.
                external = os.environ.get("CUA_E2E_EXTERNAL", "1") != "0"
                create = {"browser": True, "name": name}
                if cloud:
                    create["on"] = "cloud"
                if external:
                    create["url"] = "https://example.com"
                # region docs:create
                sb = await agent.ok("sandbox_create", create)
                space = sb["id"]
                ids = {
                    "session": sb["session"],
                    "target_id": sb["browser"]["target_id"],
                    "tab_id": sb["browser"]["tab_id"],
                }
                # endregion docs:create
                result["create_seconds"] = agent.transcript[-1]["seconds"]

                # region docs:helpers
                async def driver(tool: str, **args) -> dict:
                    return await agent.ok(
                        "call_tool", {"space": space, "tool": tool, "arguments": {**ids, **args}}
                    )

                async def read() -> dict:
                    return await driver("get_browser_state", snapshot_format="semantic_v2")

                async def click(ref: str) -> None:
                    # In a sandbox nobody else uses the browser window, so the
                    # trusted route may activate it (Linux Chromium needs that).
                    await driver("browser_click", ref=ref, delivery_mode="foreground")

                # endregion docs:helpers

                # 3. Read a public page.
                if external:
                    page = await read()
                    find_ref(page, "heading", "Example Domain")
                    result["external_page"] = page.get("page", {}).get("url")

                # 4. Serve a small form inside the sandbox and fill it.
                await agent.ok(
                    "space_write",
                    {
                        "space": space,
                        "path": "/tmp/form_server.py",
                        "content": FORM_SERVER.read_text(),
                    },
                )
                await agent.ok(
                    "space_bash",
                    {
                        "space": space,
                        "command": f"nohup python3 /tmp/form_server.py {FORM_PORT} >/tmp/form.log 2>&1 &"
                        f" for i in $(seq 1 50); do curl -sf 127.0.0.1:{FORM_PORT}/ >/dev/null && break; sleep 0.2; done",
                    },
                )
                # region docs:fill
                await driver("browser_navigate", url=f"http://127.0.0.1:{FORM_PORT}/")
                # One read; its refs stay valid until the page changes.
                page = await read()
                name_ref = find_ref(page, "textbox", "Name")
                agree_ref = find_ref(page, "checkbox", "I agree")
                submit_ref = find_ref(page, "button", "Submit")
                await driver("browser_type", ref=name_ref, text="Ada Lovelace", replace=True)
                await click(agree_ref)
                await click(submit_ref)
                # endregion docs:fill

                # 5. Verify the submission landed (bounded polling).
                for _ in range(MAX_READS):
                    page = await read()
                    if "Thanks, Ada Lovelace" in page.get("outline", ""):
                        break
                    await asyncio.sleep(0.25)
                else:
                    raise Step(f"the form did not submit: {page.get('outline')}")
                if "agree=yes" not in page.get("outline", ""):
                    raise Step(f"the checkbox value did not submit: {page.get('outline')}")
                result["submitted"] = page.get("page", {}).get("url")

                # 6. Screenshot of the tab.
                _, content, err = await agent.call(
                    "call_tool",
                    {
                        "space": space,
                        "tool": "get_browser_state",
                        "arguments": {
                            **ids,
                            "snapshot_format": "semantic_v2",
                            "include_screenshot": True,
                        },
                    },
                )
                shots = [c for c in content if c.type == "image"]
                if err or not shots:
                    raise Step("no screenshot came back")
                out.mkdir(parents=True, exist_ok=True)
                png = out / "browse-result.png"
                png.write_bytes(base64.b64decode(shots[0].data))
                result["screenshot"] = str(png)
            finally:
                # 7. Clean up, always.
                if space:
                    # region docs:cleanup
                    await agent.call("sandbox_delete", {"name": space})
                    # endregion docs:cleanup
                result["transcript"] = agent.transcript
                result["tool_calls"] = len(agent.transcript)
                result["wall_seconds"] = round(time.monotonic() - started, 2)
    return result


def main() -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--cloud", action="store_true", help="run the sandbox on Cua Fleet (gVisor)")
    a = p.parse_args()
    if os.environ.get("CUA_E2E_BROWSE") != "1":
        print("skipped: set CUA_E2E_BROWSE=1 (starts a sandbox)")
        return 0
    out = pathlib.Path(os.environ.get("CUA_E2E_OUT", "."))
    result: dict = {}
    try:
        asyncio.run(run(a.cloud, out, result))
    except BaseException as e:  # Step, or an ExceptionGroup around one
        leaves = [e]
        while leaves and isinstance(leaves[0], BaseExceptionGroup):
            leaves = list(leaves[0].exceptions)
        print(f"FAILED: {leaves[0] if leaves else e}", file=sys.stderr)
        out.mkdir(parents=True, exist_ok=True)
        (out / "transcript.json").write_text(json.dumps(result, indent=2))
        return 1
    out.mkdir(parents=True, exist_ok=True)
    (out / "transcript.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != "transcript"}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
