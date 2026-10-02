"""Accessibility + input probe for the macOS image (build and publish gate).

The doctor's effect checks need GTK fixtures that macOS does not have, so
this probe proves the TCC grants the image seeds actually work, through the
image's own cua-spacesd /mcp endpoint (the cua-driver tools linked into it),
with the official MCP Python SDK:

  1. check_permissions (read-only): Accessibility and Screen Recording granted
  2. launch_app Calculator in the background
  3. get_window_state: the AX tree of its window (needs Accessibility)
  4. click 7, +, 2, = as CGEvent clicks at the keys' AX frames (needs
     Accessibility / PostEvent; each click follows a fresh screenshot, as
     the pixel path requires)
  5. get_window_state again: the display (an AXStaticText) reads 9
  6. kill_app
  7. move_cursor with a session key: the agent cursor animates and must
     come back (a daemon whose overlay never ran waited forever here and
     then wedged every later call)

Element tokens are not used: over /mcp each call runs in its own driver
runtime scope, so a token from one call is refused by the next.

  uv run --with 'mcp>=1.12,<1.14' python ax_probe.py URL TOKEN_FILE [OUT.json]

Exit 0 on success. Writes a JSON record of every step to OUT.json.
"""

from __future__ import annotations

import asyncio
import json
import re
import sys
from typing import Any

from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

CALCULATOR = "com.apple.calculator"
# Calculator's AX labels for the keys, by macOS release.
KEYS = {
    "7": ("7",),
    "+": ("Add", "+", "plus"),
    "2": ("2",),
    "=": ("Equals", "=", "equals"),
}


def structured(result: Any) -> dict[str, Any]:
    if getattr(result, "isError", False):
        text = " ".join(getattr(c, "text", "") for c in result.content)
        raise RuntimeError(text or "tool error")
    data = getattr(result, "structuredContent", None)
    if isinstance(data, dict):
        return data
    for item in result.content:
        text = getattr(item, "text", "")
        try:
            return json.loads(text)
        except (TypeError, ValueError):
            continue
    return {}


def find_key(elements: list[dict[str, Any]], labels: tuple[str, ...]) -> dict[str, Any] | None:
    for element in elements:
        if "button" not in str(element.get("role", "")).lower():
            continue
        names = {str(element.get(k, "")).strip() for k in ("label", "title", "description", "identifier")}
        if any(label in names for label in labels):
            return element
    return None


def normalize(text: str) -> str:
    """Calculator wraps its display in bidi marks; drop them and separators."""
    return "".join(ch for ch in text if ch.isalnum() or ch in ".-")


def display_text(state: dict[str, Any]) -> str:
    """The window's static texts (Calculator's display), from the tree
    rendering: `elements` lists only actionable nodes."""
    texts = re.findall(r'AXStaticText = "([^"]*)"', str(state.get("tree_markdown", "")))
    return " | ".join(t for t in texts if t)


async def run(url: str, token: str, log: list[dict[str, Any]]) -> bool:
    headers = {"Authorization": f"Bearer {token}"}
    async with streamablehttp_client(url, headers=headers) as (read, write, _):
        async with ClientSession(read, write) as session:
            await session.initialize()

            async def call(name: str, args: dict[str, Any]) -> dict[str, Any]:
                data = structured(await session.call_tool(name, args))
                log.append({"tool": name, "args": args, "result_keys": sorted(data)})
                return data

            perms = await call("check_permissions", {"prompt": False})
            log[-1]["result"] = {k: perms.get(k) for k in ("accessibility", "screen_recording")}
            if not (perms.get("accessibility") and perms.get("screen_recording")):
                raise RuntimeError(f"permissions not granted: {log[-1]['result']}")

            launched = await call("launch_app", {"bundle_id": CALCULATOR})
            pid = launched.get("pid") or launched.get("process_id")
            if not pid:
                raise RuntimeError(f"launch_app returned no pid: {launched}")
            try:
                # A background launch maps its window before AX is populated,
                # and an app can own several CG windows: take the first
                # standard window whose AX tree resolves.
                state, base, elements, origin = {}, {}, [], (0, 0)
                for _ in range(30):
                    listed = await call("list_windows", {"pid": pid})
                    candidates = [
                        w for w in listed.get("windows", [])
                        if w.get("pid", pid) == pid and w.get("window_id")
                        and not w.get("layer") and w.get("is_on_screen", True)
                    ]
                    candidates.sort(key=lambda w: -(w.get("bounds", {}).get("width", 0)
                                                    * w.get("bounds", {}).get("height", 0)))
                    for w in candidates:
                        base = {"pid": pid, "window_id": w["window_id"]}
                        origin = (w.get("bounds", {}).get("x", 0), w.get("bounds", {}).get("y", 0))
                        state = await call("get_window_state", {**base, "include_screenshot": False})
                        elements = state.get("elements", [])
                        log[-1]["elements"] = len(elements)
                        if elements:
                            break
                    if elements:
                        break
                    await asyncio.sleep(0.5)
                if not elements:
                    raise RuntimeError(
                        f"empty accessibility tree: {state.get('degraded_reason') or state.get('_note')}")

                def target(element: dict[str, Any]) -> dict[str, Any]:
                    # Pixel path: a CGEvent click at the element's centre in
                    # window-local points (needs Accessibility / PostEvent).
                    # AX frames are global screen points.
                    f = element.get("frame") or {}
                    return {"x": f["x"] + f["w"] / 2 - origin[0], "y": f["y"] + f["h"] / 2 - origin[1]}

                clear = find_key(elements, ("All Clear", "Clear", "AC", "C"))
                state = await call("get_window_state", base)
                if clear:
                    await call("click", {**base, **target(clear)})
                    state = await call("get_window_state", base)
                for key in ("7", "+", "2", "="):
                    element = find_key(state.get("elements", []), KEYS[key])
                    if element is None:
                        raise RuntimeError(f"no {key!r} key in the AX tree")
                    await call("click", {**base, **target(element)})
                    state = await call("get_window_state", base)
                shown = display_text(state)
                log.append({"display": shown})
                calculated = any(normalize(t) == "9" for t in shown.split(" | "))
            finally:
                try:
                    await call("kill_app", {"pid": pid})
                except Exception as error:  # the probe result stands
                    log.append({"kill_app": str(error)})
            # A session-keyed cursor move animates the agent cursor and waits
            # for it to arrive; it must return promptly.
            try:
                await asyncio.wait_for(
                    call("move_cursor", {"x": 200, "y": 200, "session": "ax-probe"}), timeout=30)
            except asyncio.TimeoutError:
                raise RuntimeError("move_cursor with a session never returned (agent cursor overlay)")
            return calculated


def main() -> int:
    url, token_file = sys.argv[1], sys.argv[2]
    out = sys.argv[3] if len(sys.argv) > 3 else None
    token = open(token_file, encoding="utf-8").read().strip()
    log: list[dict[str, Any]] = []
    try:
        ok = asyncio.run(asyncio.wait_for(run(url, token, log), timeout=180))
        error = "" if ok else "Calculator did not show 9 after 7 + 2 ="
    except Exception as exc:  # report, never hang
        while isinstance(exc, BaseExceptionGroup) and exc.exceptions:
            exc = exc.exceptions[0]
        ok, error = False, f"{type(exc).__name__}: {exc}"
    record = {"ok": ok, "error": error, "steps": log}
    if out:
        with open(out, "w", encoding="utf-8") as fh:
            json.dump(record, fh, indent=2, default=str)
    print(("ax-probe: pass" if ok else f"ax-probe: FAIL {error}") + f" ({len(log)} steps)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
