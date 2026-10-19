"""OSWorld smoke hooks (loaded by scripts/bench-images/smoke.py)."""

from __future__ import annotations

import asyncio


async def server(sb, method: str, path: str, body: dict | None = None, timeout: float = 120):
    return await sb.service("server").request(method, path, json=body, timeout=timeout)


async def input_probe(sb, evidence, tag, screen_diff):
    """Open the GNOME overview with the Activities button (top-left) through
    cua-driver, check the screen changed, then close it with Escape."""
    before = await sb.screenshot()
    await sb.mouse.click(40, 12)
    await asyncio.sleep(2.5)
    after = await sb.screenshot()
    (evidence / f"{tag}-input-after.png").write_bytes(after)
    changed = screen_diff(before, after)
    await sb.keyboard.keypress("Escape")
    await asyncio.sleep(1.5)
    return {"ok": changed > 0.05, "changed_fraction": round(changed, 3)}


async def bench_checks(sb, evidence, tag):
    out = []
    r = await server(sb, "GET", "/screenshot")
    ok = r.status_code == 200 and r.content[:8] == b"\x89PNG\r\n\x1a\n"
    if ok:
        (evidence / f"{tag}-osworld-screenshot.png").write_bytes(r.content)
    out.append(("server_screenshot", ok, f"{r.status_code}, {len(r.content)} bytes"))
    r = await server(sb, "POST", "/execute", {"command": ["bash", "-c", "id -un; echo $DISPLAY"], "shell": False})
    body = r.json() if r.status_code == 200 else {}
    out.append(("server_execute", r.status_code == 200 and "user" in str(body.get("output", "")), body))
    r = await server(sb, "GET", "/accessibility", timeout=180)
    at = r.json().get("AT", "") if r.status_code == 200 else ""
    out.append(("server_accessibility", r.status_code == 200 and len(at) > 1000,
                f"{r.status_code}, {len(at)} chars" + ("" if r.status_code == 200 else f": {r.text[:300]}")))
    r = await server(sb, "POST", "/screen_size")
    out.append(("server_screen_size", r.status_code == 200 and r.json().get("width") == 1920, r.text[:100]))
    return out
