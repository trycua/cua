import asyncio
import json
import os
import random
import socket
import sys
import threading
from pathlib import Path
from typing import Optional

import webview
from aiohttp import web


def _get_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _start_http_server(window: webview.Window, port: int, ready_event: threading.Event):
    async def rect_handler(request: web.Request):
        try:
            data = await request.json()
        except Exception:
            return web.json_response({"error": "invalid_json"}, status=400)
        selector = data.get("selector")
        space = data.get("space", "window")
        if not isinstance(selector, str):
            return web.json_response({"error": "selector_required"}, status=400)

        # Ensure window content is loaded
        if not ready_event.is_set():
            # give it a short chance to finish loading
            ready_event.wait(timeout=2.0)
        if not ready_event.is_set():
            return web.json_response({"error": "window_not_ready"}, status=409)

        # Safely embed selector into JS
        selector_js = json.dumps(selector)
        if space == "screen":
            # Compute approximate screen coordinates using window metrics
            js = (
                "(function(){"
                f"const s = {selector_js};"
                "const el = document.querySelector(s);"
                "if(!el){return null;}"
                "const r = el.getBoundingClientRect();"
                "const sx = (window.screenX ?? window.screenLeft ?? 0);"
                "const syRaw = (window.screenY ?? window.screenTop ?? 0);"
                "const frameH = (window.outerHeight - window.innerHeight) || 0;"
                "const sy = syRaw + frameH;"
                "return {x:sx + r.left, y:sy + r.top, width:r.width, height:r.height};"
                "})()"
            )
        else:
            js = (
                "(function(){"
                f"const s = {selector_js};"
                "const el = document.querySelector(s);"
                "if(!el){return null;}"
                "const r = el.getBoundingClientRect();"
                "return {x:r.left,y:r.top,width:r.width,height:r.height};"
                "})()"
            )
        try:
            # Evaluate JS on the target window; this call is thread-safe in pywebview
            result = window.evaluate_js(js)
        except Exception as e:
            return web.json_response({"error": str(e)}, status=500)
        return web.json_response({"rect": result})

    async def eval_handler(request: web.Request):
        try:
            data = await request.json()
        except Exception:
            return web.json_response({"error": "invalid_json"}, status=400)
        code = data.get("javascript") or data.get("code")
        if not isinstance(code, str):
            return web.json_response({"error": "javascript_required"}, status=400)

        if not ready_event.is_set():
            ready_event.wait(timeout=2.0)
        if not ready_event.is_set():
            return web.json_response({"error": "window_not_ready"}, status=409)

        try:
            result = window.evaluate_js(code)
        except Exception as e:
            return web.json_response({"error": str(e)}, status=500)
        return web.json_response({"result": result})

    app = web.Application()
    app.router.add_post("/rect", rect_handler)
    app.router.add_post("/eval", eval_handler)

    loop = asyncio.new_event_loop()

    def run_loop():
        asyncio.set_event_loop(loop)
        runner = web.AppRunner(app)
        loop.run_until_complete(runner.setup())
        site = web.TCPSite(runner, "127.0.0.1", port)
        loop.run_until_complete(site.start())
        loop.run_forever()

    t = threading.Thread(target=run_loop, daemon=True)
    t.start()


def main():
    if len(sys.argv) < 2:
        print("Usage: python -m bench_ui.child <config.json>", file=sys.stderr)
        sys.exit(2)

    cfg_path = Path(sys.argv[1])
    cfg = json.loads(cfg_path.read_text(encoding="utf-8"))

    html: Optional[str] = cfg.get("html") or ""
    url: Optional[str] = cfg.get("url")
    title: str = cfg.get("title", "Window")
    x: Optional[int] = cfg.get("x")
    y: Optional[int] = cfg.get("y")
    width: int = int(cfg.get("width", 600))
    height: int = int(cfg.get("height", 400))
    icon: Optional[str] = cfg.get("icon")
    use_inner_size: bool = bool(cfg.get("use_inner_size", False))
    title_bar_style: str = cfg.get("title_bar_style", "default")

    # Create window
    if url:
        window = webview.create_window(
            title,
            url=url,
            width=width,
            height=height,
            x=x,
            y=y,
            confirm_close=False,
            text_select=True,
            background_color="#FFFFFF",
        )
    else:
        window = webview.create_window(
            title,
            html=html,
            width=width,
            height=height,
            x=x,
            y=y,
            confirm_close=False,
            text_select=True,
            background_color="#FFFFFF",
        )

    # Track when the page is loaded so JS execution succeeds
    window_ready = threading.Event()
    def _on_loaded():
        window_ready.set()
    window.events.loaded += _on_loaded  # type: ignore[attr-defined]

    # Start HTTP server for control
    port = _get_free_port()
    _start_http_server(window, port, window_ready)

    # Print startup info for parent to read
    print(json.dumps({"pid": os.getpid(), "port": port}), flush=True)

    # Start GUI (blocking)
    webview.start()


if __name__ == "__main__":
    main()
