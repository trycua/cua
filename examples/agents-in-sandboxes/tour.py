#!/usr/bin/env python3
"""The "Run a coding agent in a sandbox" docs page, as one runnable script.

The page's Python blocks are the `docs:<id>` regions below, byte for byte
(`test="excerpt"`, checked by tests/e2e/cua-sdk/docs/extract.py --lint).

    ANTHROPIC_API_KEY=... python tour.py                      # a real key
    ANTHROPIC_API_KEY=$MOCK_KEY OPENAI_API_KEY=$MOCK_KEY python tour.py \
        --base-url http://<mock-llm>:8787                  # the mock provider

With --base-url every run goes to that endpoint (the scripted mock provider,
cua-mock-llm, in tests); that setup is not part of the page.
"""

from __future__ import annotations

import argparse
import asyncio
import functools
import time

import cua

NAME = "agents-demo"


# region docs:follow
async def follow(run, cursor=0, timeout=600):
    """Prints events until the turn ends; returns the cursor to continue from."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        page = await run.events(cursor, None)
        cursor = page.cursor
        for e in page.events:
            if e.line:
                print(f"{e.kind:12} {e.line.splitlines()[0][:100]}")
            if e.kind in ("turn_ended", "exited"):
                return cursor
        if page.caught_up:
            await asyncio.sleep(0.5)
    raise TimeoutError(f"{run.run_id()} is still working")
# endregion docs:follow


async def main(image: str, name: str) -> None:
    # region docs:create
    c = cua.embedded()
    sb = await c.sandboxes().create(cua.SandboxCreateOptions(
        on="local",                               # or "cloud"
        image=image,                              # any image with cua-spacesd
        name=name,
        memory_mb=4096,
        wait_for=[cua.ReadinessProbe(service="env")],
    ))
    # endregion docs:create
    try:
        await tour(sb)
    finally:
        await sb.delete()


async def tour(sb) -> None:
    # region docs:run
    agents = await sb.agents()
    run = await agents.run(
        "claude-code",
        "Write primes.py that prints the first 10 primes, then run it.",
        cua.AgentRunOptions(env_from_host=["ANTHROPIC_API_KEY"]),
    )
    print(run.run_id())
    # endregion docs:run

    # region docs:stream
    cursor = await follow(run)
    # endregion docs:stream

    # region docs:followup
    await run.send("Now add a unit test for it and run the test.", None)
    cursor = await follow(run, cursor)
    # endregion docs:followup

    # region docs:interrupt
    await run.send("Rewrite it to use a sieve and benchmark both versions.", None)
    await asyncio.sleep(5)
    await run.interrupt()      # cancels the turn in flight; the session stays open
    cursor = await follow(run, cursor)
    # endregion docs:interrupt

    # region docs:result
    result = await run.result()
    print(result.status, result.stop_reason, result.tool_calls, result.usage_json)
    print(result.text)
    for a in await run.artifacts():
        print(a.path, a.size)
    await run.stop()
    # endregion docs:result

    # region docs:reattach
    agents = await sb.agents()            # any process, later
    runs = await agents.list()            # newest first
    for info in runs:
        print(info.run_id, info.harness, info.status, info.label)
    run = await agents.get(runs[0].run_id)
    print((await run.result()).text)
    # endregion docs:reattach

    # region docs:mcp
    run = await agents.run(
        "openai-codex",
        "Use the docs server to look up our API style guide, then review api.py.",
        cua.AgentRunOptions(
            env_from_host=["OPENAI_API_KEY"],
            mcp_servers=[
                cua.AgentRunMcpServer(name="docs", url="https://mcp.example.com/mcp",
                                      headers={"Authorization": "Bearer <token>"}),
                cua.AgentRunMcpServer(name="fs", command="npx",
                                      args=["-y", "@modelcontextprotocol/server-filesystem", "/srv"]),
            ],
            exit_when_idle=True,
        ),
    )
    # endregion docs:mcp
    done = await run.wait(600_000)
    print(done.status, done.turn, done.stop_reason, done.text[:80])

    # region docs:endpoint
    run = await agents.run(
        "claude-code",
        "Summarize the repository in five bullet points.",
        cua.AgentRunOptions(
            env={"ANTHROPIC_API_KEY": "<gateway key>"},
            base_url="https://llm-gateway.internal.example.com",
            model="claude-sonnet-4-5",
            exit_when_idle=True,
        ),
    )
    # endregion docs:endpoint
    done = await run.wait(600_000)
    print(done.status, done.turn, done.stop_reason, done.text[:80])


def _mock(base_url: str, model: str | None) -> None:
    """Tests only: point every run at the mock provider whatever the page's
    options say. Keys come from this environment under the page's variable
    names (set them to the mock's key); the placeholder remote MCP server is
    dropped."""
    real = cua.AgentRunOptions

    @functools.wraps(real)
    def options(**kw):
        kw["base_url"] = base_url
        if model:
            kw["model"] = model
        env = kw.pop("env", {})
        kw["env_from_host"] = list(kw.get("env_from_host", [])) + list(env)
        kw["mcp_servers"] = [m for m in kw.get("mcp_servers", []) if m.name != "docs"]
        return real(**kw)

    cua.AgentRunOptions = options


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--image", default="ghcr.io/trycua/linux:24.04")
    ap.add_argument("--name", default=NAME)
    ap.add_argument("--base-url", help="tests: the scripted mock provider")
    ap.add_argument("--model")
    a = ap.parse_args()
    if a.base_url:
        _mock(a.base_url, a.model)
    asyncio.run(main(a.image, a.name))
