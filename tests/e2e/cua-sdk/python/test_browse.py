"""Browse the web from an agent with Cua (guide: cua-sdk/guides/browse-the-web).

A fresh-agent session driven ONLY through the host `cua mcp` server with the
official MCP Python SDK (tests/e2e/browse/e2e_browse.py), no LLM:

* hermetic: the MCP surface an agent discovers first: `images_list` returns
  the catalog the docs publish (browser_tools on the canonical Linux image),
  `sandbox_create` takes `browser`, and `teleport_browser_session` exists.
* container: images_list -> sandbox_create {browser: true} on the locally
  built linux -> read, fill and submit a form served inside the
  sandbox -> screenshot -> sandbox_delete.
* fleet-env: the same session on a Fleet gVisor sandbox of
  ``$CUA_E2E_FLEET_ENV_IMAGE``.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path

import e2e
import pytest

BROWSE = Path(__file__).resolve().parents[2] / "browse"
sys.path.insert(0, str(BROWSE))

mcp = pytest.importorskip("mcp", reason="the MCP Python SDK is not installed (pip install mcp)")
import e2e_browse  # noqa: E402

GUIDE = "cua-sdk/guides/browse-the-web.mdx"


def _cua() -> str:
    cli = e2e.cua_cli()
    if cli is None:
        pytest.skip("the cua CLI is not built")
    return str(cli)


def _env(tmp_path: Path, image: str | None) -> dict:
    env = {
        "CUA_BIN": _cua(),
        "CUA_HOME": str(tmp_path / "cua-home"),
        # Deterministic: no public site, only the page served in the sandbox.
        "CUA_E2E_EXTERNAL": os.environ.get("CUA_E2E_EXTERNAL", "0"),
    }
    if image:
        env["CUA_IMAGE_LINUX"] = image
    return env


def _run(tmp_path: Path, env: dict, cloud: bool) -> dict:
    old = {k: os.environ.get(k) for k in env}
    os.environ.update(env)
    result: dict = {}
    try:
        asyncio.run(asyncio.wait_for(e2e_browse.run(cloud, tmp_path, result), timeout=900))
    finally:
        for k, v in old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        (tmp_path / "transcript.json").write_text(json.dumps(result, indent=2))
    return result


@pytest.mark.e2e("browse-the-web", "hermetic")
def test_browse_mcp_surface(tmp_path):
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client

    async def go():
        env = dict(os.environ, CUA_HOME=str(tmp_path / "cua-home"))
        params = StdioServerParameters(command=_cua(), args=["mcp"], env=env)
        async with stdio_client(params) as (r, w):
            async with ClientSession(r, w) as s:
                await s.initialize()
                tools = {t.name: t for t in (await s.list_tools()).tools}
                for name in (
                    "images_list",
                    "sandbox_create",
                    "sandbox_open_browser",
                    "teleport_browser_session",
                    "call_tool",
                    "sandbox_delete",
                ):
                    assert name in tools, name
                t = tools["sandbox_create"]
                # MCP SDK 2.x: input_schema; 1.x: inputSchema.
                schema = getattr(t, "input_schema", None) or t.inputSchema
                assert schema["properties"]["browser"]["type"] == "boolean"
                r = await s.call_tool("images_list", {"browser": True})
                cat = json.loads(r.content[0].text)
                refs = {i["ref"]: i for i in cat["images"]}
                linux = refs["ghcr.io/trycua/linux:24.04"]
                assert linux["browser_tools"] is True
                assert "chromium" in linux["browsers"]
                assert all(i["browser_tools"] for i in cat["images"])
                # The catalog is the file the docs are generated from.
                source = json.loads(
                    (
                        Path(__file__).resolve().parents[4] / "libs/images/sandbox-images.json"
                    ).read_text()
                )
                published = {i["ref"] for i in source["images"] if i["published"]}
                assert set(refs) <= published

    asyncio.run(asyncio.wait_for(go(), timeout=120))


@pytest.mark.e2e("browse-the-web", "container")
@pytest.mark.covers_docs(
    f"{GUIDE}#browse-discover",
    f"{GUIDE}#browse-create",
    f"{GUIDE}#browse-helpers",
    f"{GUIDE}#browse-fill",
    f"{GUIDE}#browse-cleanup",
)
def test_browse_local_sandbox(tmp_path):
    image = e2e.desktop_image()
    e2e.require_image(image)
    result = _run(tmp_path, _env(tmp_path, image), cloud=False)
    assert result["submitted"].endswith("/submit?name=Ada+Lovelace&agree=yes"), result
    assert Path(result["screenshot"]).stat().st_size > 1000


@pytest.mark.e2e("browse-the-web", "fleet-env")
def test_browse_fleet_gvisor(tmp_path):
    result = _run(tmp_path, _env(tmp_path, os.environ["CUA_E2E_FLEET_ENV_IMAGE"]), cloud=True)
    assert result["submitted"].endswith("/submit?name=Ada+Lovelace&agree=yes"), result
