"""bench-web smoke hooks (loaded by scripts/bench-images/smoke.py)."""

from __future__ import annotations

import asyncio
import json

CLICK_PAGE = (
    "data:text/html,<html><body style='margin:0'>"
    "<button id=b style='width:100vw;height:100vh;font-size:64px;background:%23eee' "
    "onclick=\"document.title='clicked';this.style.background='%23c00'\">click</button></body></html>"
)


async def ctl(sb, method: str, path: str, body: dict | None = None, timeout: float = 90):
    r = await sb.service("server").request(method, path, json=body, timeout=timeout)
    if r.status_code != 200:
        raise RuntimeError(f"{method} {path} -> {r.status_code}: {r.text[:200]}")
    return r.json()


async def js(sb, expression: str):
    return (await ctl(sb, "POST", "/eval", {"expression": expression}))["value"]


async def input_probe(sb, evidence, tag, screen_diff):
    """Click the middle of a full-viewport button through cua-driver."""
    await ctl(sb, "POST", "/reset", {"url": CLICK_PAGE})
    await asyncio.sleep(2)
    before = await sb.screenshot()
    x = int(await js(sb, "window.screenX + window.outerWidth / 2"))
    y = int(await js(sb, "window.screenY + (window.outerHeight - window.innerHeight) + window.innerHeight / 2"))
    await sb.mouse.click(x, y)
    title = None
    for _ in range(20):
        title = await js(sb, "document.title")
        if title == "clicked":
            break
        await asyncio.sleep(0.25)
    await asyncio.sleep(1.0)  # let the page repaint before the capture
    after = await sb.screenshot()
    (evidence / f"{tag}-input-after.png").write_bytes(after)
    changed = screen_diff(before, after)
    return {"ok": title == "clicked" and changed > 0.05, "click": [x, y], "title": title,
            "changed_fraction": round(changed, 3)}


async def _miniwob(sb, task: str, solve: str | None):
    await ctl(sb, "POST", "/reset", {"url": f"http://127.0.0.1:7560/miniwob/{task}.html"})
    await js(sb, "core.startEpisodeReal(); true")
    await asyncio.sleep(0.5)
    if solve:
        await js(sb, solve)
        await asyncio.sleep(0.5)
    return await js(sb, "({done: WOB_DONE_GLOBAL, raw: WOB_RAW_REWARD_GLOBAL, reward: WOB_REWARD_GLOBAL})")


# A scripted solution: click the button whose label the query names.
CLICK_BUTTON_SOLVE = (
    "(() => { const want = document.querySelector('#query').textContent.match(/\"(.*)\"/)[1];"
    " for (const b of document.querySelectorAll('#area button')) if (b.textContent.trim() === want) b.click();"
    " return want; })()"
)

BENCH_UI_PY = r"""
import json, time
from bench_ui import launch_window, get_element_rect, execute_javascript
pid = launch_window(html="<html><body><button id='b' onclick='window.__ok=1'>OK</button></body></html>",
                    title="bench-ui smoke", width=320, height=240)
rect = None
for _ in range(40):
    rect = get_element_rect(pid, "#b", space="screen")
    if rect: break
    time.sleep(0.25)
print("RESULT" + json.dumps({"pid": pid, "rect": rect, "two": execute_javascript(pid, "1+1")}))
"""


async def bench_checks(sb, evidence, tag):
    out = []
    solved = await _miniwob(sb, "click-button", CLICK_BUTTON_SOLVE)
    out.append(("miniwob_oracle", bool(solved.get("done")) and solved.get("raw", 0) > 0, solved))
    noop = await _miniwob(sb, "click-button", None)
    out.append(("miniwob_noop", not noop.get("done") and noop.get("raw", 0) <= 0, noop))
    health = await ctl(sb, "GET", "/healthz")
    out.append(("chromium", bool(health.get("ok")), health))
    r = await sb.shell.run("python3 - <<'PY'\n" + BENCH_UI_PY + "\nPY", timeout=120)
    text = getattr(r, "stdout", "") or ""
    line = next((l for l in text.splitlines() if l.startswith("RESULT")), None)
    res = json.loads(line[6:]) if line else {"stdout": text[-300:], "stderr": (getattr(r, "stderr", "") or "")[-300:]}
    out.append(("bench_ui_window", bool(line) and res.get("rect") is not None and res.get("two") == 2, res))
    return out
