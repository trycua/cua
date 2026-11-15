#!/usr/bin/env python3
"""The `agent` conformance case: a coding agent session starts in a Space and answers.

Installs the coding app in the guest (a real download), serves the scripted model next to it (the
guest-side endpoint the bench uses), adds a standby agent and waits for its session, then sends one
turn. Exit 0 when the scripted reply arrives. Driven by `run.py`; needs a python with `mcp`.
"""
import argparse, asyncio, base64, os, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "cua-bench" / "datasets" / "ask-first"))
import askfirst_bench as ab  # noqa: E402


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cua", required=True)
    ap.add_argument("--space", required=True, help="local:<name>")
    ap.add_argument("--name", required=True, help="the sandbox name")
    ap.add_argument("--harness", default="claude-code")
    a = ap.parse_args()
    os.environ["CUA"] = a.cua
    os.environ.setdefault("ANTHROPIC_API_KEY", "mock-key")  # forwarded to the agent; the scripted endpoint ignores it
    t0 = time.monotonic()
    rc, o, e = await ab.cua("agent", "ensure", a.space, a.harness, "node", timeout=900)
    if rc != 0:
        sys.exit(f"agent ensure failed ({rc}): {(e or o).strip()[:200]}")
    t_ensure = time.monotonic() - t0
    b64 = base64.b64encode(ab.SCRIPTED_LLM.read_text().encode()).decode()
    await ab.cua("sandbox", "exec", a.name, f"echo {b64} | base64 -d > {ab.GUEST_LLM}", timeout=60)
    await ab.cua("sandbox", "exec", a.name, f"rm -f {ab.GUEST_LOG}; (setsid nohup node {ab.GUEST_LLM} --port {ab.PORT} --log {ab.GUEST_LOG} >/tmp/scripted_llm.out 2>&1 &) ; sleep 1", timeout=60)
    rc, o, _ = await ab.cua("sandbox", "exec", a.name, f"curl -sS -m 2 http://127.0.0.1:{ab.PORT}/healthz", timeout=60)
    if "ok" not in o:
        sys.exit("the scripted model did not start in the guest")
    member = f"conf-{int(time.time()) % 100000}"
    space = a.space.split(":", 1)[1]
    async with ab.CuaMcp({}) as mcp:
        t1 = time.monotonic()
        await mcp.call("agent_add", {"space": space, "name": member, "agent": a.harness, "base_url": f"http://127.0.0.1:{ab.PORT}",
                                     "env_from_host": ["ANTHROPIC_API_KEY"], "standby": True})
        cursor, deadline, ready = 0, time.monotonic() + 600, None
        while time.monotonic() < deadline:
            try:
                page = await mcp.call("agent_events", {"space": space, "name": member, "cursor": cursor})
            except RuntimeError:
                await asyncio.sleep(0.25)
                continue
            if isinstance(page, dict):
                cursor = page.get("cursor", cursor)
                if any(ev.get("kind") == "session" for ev in page.get("events", [])):
                    ready = time.monotonic() - t1
                    break
            await asyncio.sleep(0.25)
        if ready is None:
            sys.exit("the agent session did not start in 600 s")
        r = await ab.member_turn(mcp, space, member, "mock: delay 300; say conformance-ok", cursor, 1, poll_s=0.05, timeout=300, until_end=True, fresh_only=True)
        if not r.get("t_first"):
            sys.exit("the agent did not answer")
        total = (r["t_first"] - r["t0"]) * 1000
    print(f"agent ok: app installed in {t_ensure:.1f}s, session in {ready:.1f}s, first reply {total:.0f} ms")


asyncio.run(main())
